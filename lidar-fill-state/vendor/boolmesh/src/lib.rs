//--- Copyright (C) 2025 Saki Komikado <komietty@gmail.com>,
//--- This Source Code Form is subject to the terms of the Mozilla Public License v.2.0.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::cast_abs_to_unsigned)]
#![allow(unused_braces)]
#![no_std]

extern crate alloc;

mod manifold;
mod triangulation;
mod simplification;
mod common;
mod values;
mod boolean03;
mod boolean45;
mod tests;

use alloc::string::String;

use crate::boolean03::boolean03;
use crate::boolean45::boolean45;
use crate::simplification::simplify_topology;
use crate::triangulation::triangulate;
use crate::common::*;
use crate::values::*;
use crate::manifold::*;

pub use crate::common::{Real, Vec2, Vec3, Vec4, Mat3, K_PRECISION};

pub mod prelude {
    pub use crate::common::OpType;
    pub use crate::manifold::Manifold;
    pub use crate::compute_boolean;
}

pub trait Data: Clone + Send + Sync + core::fmt::Debug + PartialEq + Default + core::ops::Add<Output = Self> + core::ops::Mul<Real, Output = Self> {}
impl<T> Data for T where T: Clone + Send + Sync + core::fmt::Debug + PartialEq + Default + core::ops::Add<Output = T> + core::ops::Mul<Real, Output = T> {}


pub fn compute_boolean<S: Data>(
    mp: &Manifold<S>,
    mq: &Manifold<S>,
    op: OpType,
) -> Result<Manifold<S>, String> {
    let eps = mp.eps.max(mq.eps);
    let tol = mp.tol.max(mq.tol);

    let     b03 = boolean03(mp, mq, &op);
    let mut b45 = boolean45(mp, mq, &b03, &op);
    let mut trg = triangulate(mp, mq, &b45, eps)?;
    let mut val = annotate(&mut trg.hs, &b45.ps, &trg.rs, mp, mq, eps);

    simplify_topology(
        &mut trg.hs,
        &mut b45.ps,
        &mut trg.ns,
        &mut trg.rs,
        &mut val,
        b45.nv_from_p,
        b45.nv_from_q,
        eps
    );

    cleanup_unused_verts(
        &mut b45.ps,
        &mut trg.hs
    );

    Manifold::new_impl(
        b45.ps,
        trg.hs,
        val,
        Some(eps),
        Some(tol)
    )
}





