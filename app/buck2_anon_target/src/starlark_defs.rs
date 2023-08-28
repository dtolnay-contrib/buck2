/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;

use allocative::Allocative;
use buck2_build_api::artifact_groups::promise::PromiseArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::StarlarkPromiseArtifact;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::context::ANALYSIS_ACTIONS_METHODS_ANON_TARGET;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter_for_build::rule::FrozenRuleCallable;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::environment::MethodsStatic;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::dict::Dict;
use starlark::values::dict::DictOf;
use starlark::values::starlark_value;
use starlark::values::AllocValue;
use starlark::values::FrozenStringValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark_map::small_map::SmallMap;
use thiserror::Error;

use crate::anon_targets::AnonTargetsRegistry;

#[derive(Debug, Error)]
pub enum AnonTargetsError {
    #[error("artifact with name `{0}` was not found")]
    ArtifactNotFound(String),
}

#[derive(Debug, NoSerialize, ProvidesStaticType, Trace, Allocative, Clone)]
struct StarlarkAnonTarget<'v> {
    // Promise created by the anon rule
    promise: ValueTyped<'v, StarlarkPromise<'v>>,
    // Promised artifacts of the anon rule
    artifacts: SmallMap<FrozenStringValue, PromiseArtifact>,
    // Where the anon target was declared
    declaration_location: Option<FileSpan>,
}

impl<'v> Display for StarlarkAnonTarget<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<anon target")?;
        if let Some(location) = &self.declaration_location {
            write!(f, " declared at {}", location)?;
        }
        write!(f, ">")?;
        Ok(())
    }
}

#[starlark_value(type = "AnonTarget", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkAnonTarget<'v> {
    fn get_methods() -> Option<&'static Methods> {
        static RES: MethodsStatic = MethodsStatic::new();
        RES.methods(anon_target_methods)
    }
}

impl<'v> AllocValue<'v> for StarlarkAnonTarget<'v> {
    fn alloc_value(self, heap: &'v Heap) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

/// Accessors to the promise of the anon rule and the promised artifacts associated with the rule.
#[starlark_module]
fn anon_target_methods(builder: &mut MethodsBuilder) {
    /// Returns a dict where the key is the promise artifact's name, and the value is the `StarlarkPromiseArtifact`.
    fn artifacts<'v>(
        this: &StarlarkAnonTarget<'v>,
        eval: &mut Evaluator<'v, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(eval.heap().alloc(Dict::new(
            this.artifacts
                .iter()
                .map(|(k, v)| {
                    Ok((
                        k.to_value().get_hashed()?,
                        eval.heap().alloc(StarlarkPromiseArtifact::new(
                            eval.call_stack_top_location(),
                            v.clone(),
                            None,
                        )),
                    ))
                })
                .collect::<anyhow::Result<_>>()?,
        )))
    }

    /// Gets a specific `StarlarkPromiseArtifact` by name. Returns an error if the name was not found in the
    /// registered promise artifacts for the anon target.
    fn artifact<'v>(
        this: &StarlarkAnonTarget<'v>,
        name: &'v str,
        eval: &mut Evaluator<'v, '_>,
    ) -> anyhow::Result<Value<'v>> {
        match this.artifacts.get(name) {
            Some(v) => Ok(eval.heap().alloc(StarlarkPromiseArtifact::new(
                eval.call_stack_top_location(),
                v.clone(),
                None,
            ))),
            None => Err(AnonTargetsError::ArtifactNotFound(name.to_owned()).into()),
        }
    }

    /// Returns the promise that maps to the result of the anon rule.
    #[starlark(attribute)]
    fn promise<'v>(
        this: &StarlarkAnonTarget<'v>,
    ) -> anyhow::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        Ok(this.promise)
    }
}

#[starlark_module]
fn analysis_actions_methods_anon_target(builder: &mut MethodsBuilder) {
    /// An anonymous target is defined by the hash of its attributes, rather than its name.
    /// During analysis, rules can define and access the providers of anonymous targets before producing their own providers.
    /// Two distinct rules might ask for the same anonymous target, sharing the work it performs.
    ///
    /// For more details see https://buck2.build/docs/rule_authors/anon_targets/
    fn anon_target<'v>(
        this: &AnalysisActions<'v>,
        rule: ValueTyped<'v, FrozenRuleCallable>,
        attrs: DictOf<'v, &'v str, Value<'v>>,
        heap: &'v Heap,
    ) -> anyhow::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        let res = heap.alloc_typed(StarlarkPromise::new_unresolved());
        let mut this = this.state();
        let registry = AnonTargetsRegistry::downcast_mut(&mut *this.anon_targets)?;
        let key = registry.anon_target_key(rule, attrs)?;
        registry.register_one(res, key)?;
        Ok(res)
    }

    /// Generate a series of anonymous targets
    fn anon_targets<'v>(
        this: &AnalysisActions<'v>,
        rules: Vec<(
            ValueTyped<'v, FrozenRuleCallable>,
            DictOf<'v, &'v str, Value<'v>>,
        )>,
        heap: &'v Heap,
    ) -> anyhow::Result<ValueTyped<'v, StarlarkPromise<'v>>> {
        let res = heap.alloc_typed(StarlarkPromise::new_unresolved());
        let mut this = this.state();
        AnonTargetsRegistry::downcast_mut(&mut *this.anon_targets)?.register_many(res, rules)?;
        Ok(res)
    }
}

pub(crate) fn init_analysis_actions_methods_anon_target() {
    ANALYSIS_ACTIONS_METHODS_ANON_TARGET.init(analysis_actions_methods_anon_target);
}
