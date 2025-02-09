/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use allocative::Allocative;

/// Describing data that can be stored in `SelfRef`.
pub(crate) trait RefData: 'static {
    type Data<'a>: 'a;
}

/// Self-referential struct.
#[derive(Allocative)]
#[allocative(bound = "O: Allocative, D: RefData")]
pub(crate) struct SelfRef<O, D: RefData> {
    #[allocative(skip)] // TODO(nga): do not skip.
    data: D::Data<'static>,
    // Owner must be placed after `data` to ensure that `data` is dropped before `owner`.
    // Owner must be in `Arc` (or `Rc`) because
    // - pointers stay valid when `SelfRef` is moved.
    // - it cannot be `Box` because it would violate aliasing rules
    owner: Arc<O>,
}

impl<O: Debug, D: RefData> Debug for SelfRef<O, D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfRef")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl<O, D: RefData> SelfRef<O, D> {
    pub(crate) fn try_new(
        owner: O,
        data: impl for<'a> FnOnce(&'a O) -> anyhow::Result<D::Data<'a>>,
    ) -> anyhow::Result<Self> {
        let owner = Arc::new(owner);
        let data = data(&owner)?;
        let data = unsafe { std::mem::transmute::<D::Data<'_>, D::Data<'static>>(data) };
        Ok(SelfRef { owner, data })
    }

    #[inline]
    pub(crate) fn data(&self) -> &D::Data<'_> {
        unsafe { std::mem::transmute::<&D::Data<'static>, &D::Data<'_>>(&self.data) }
    }
}
