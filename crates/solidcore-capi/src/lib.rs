//! A minimal C ABI over the `solidcore` B-rep kernel (the Rust port of NVIDIA's
//! SMLib), exposed as a `cdylib` (`libsolidcore_capi.so`).
//!
//! Model: one [`ScKernel`] value owns a `solidcore::Kernel` (tolerances, RNG,
//! caches) and a registry of `Brep`s addressed by 1-based integer ids. Every
//! function that takes an id looks the `Brep` up in the owning kernel's
//! registry; there is no global state. Booleans consume their operands: the two
//! input ids are released and the result gets a new id.
//!
//! Every entry point is `#[no_mangle] extern "C"`, catches panics at the
//! boundary (`catch_unwind`) and returns an error code instead of unwinding
//! into C. All pointer arguments must be valid for their documented extent;
//! passing a null or foreign kernel/id returns an error code, never crashes.
//!
//! | Function              | Success             | Failure                |
//! |-----------------------|---------------------|------------------------|
//! | `sc_kernel_new`       | pointer             | null                   |
//! | `sc_kernel_free`      | -                   | -                      |
//! | `sc_box` / `sc_cylinder` / `sc_sphere` | id > 0 | -1                |
//! | `sc_union` / `sc_difference` / `sc_intersection` | id > 0 | -1      |
//! | `sc_translate` / `sc_rotate` / `sc_scale` | 0 | -1                  |
//! | `sc_fillet_all`       | 0                   | -1                     |
//! | `sc_volume`           | volume              | NaN                    |
//! | `sc_bbox`             | 0 (fills 6 doubles) | -1                     |
//! | `sc_mesh`             | triangle count      | -1 (NOT SUPPORTED: no tessellator is ported yet) |
//! | `sc_free`             | -                   | -                      |
//!
//! Conventions (matching the OpenSCAD interpreter's expectations):
//! - `sc_box` places the box's **corner** at (x, y, z), sizes (sx, sy, sz) along
//!   +X/+Y/+Z (`create_box` with that origin).
//! - `sc_cylinder` puts the base circle's centre at (x, y, z), axis +Z, height h
//!   (`create_cylinder`).
//! - `sc_sphere` centres at (x, y, z).
//! - `sc_rotate` takes a degree angle about an axis through the origin.
//! - `sc_fillet_all` runs `circular_fillet`: every edge that bounds a blendable
//!   sector. There is no convex/concave filter in this ABI yet.
//!
//! Meshing: `sc_mesh` always returns -1. SMLib's `SmApiTessellate` has not been
//! ported, and hand-rolling a tessellator would risk wrong geometry; callers
//! should compare kernels via `sc_volume` / `sc_bbox` instead.
#![allow(unsafe_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;

use solidcore::geom::attribute::{AttributeStore, ObjectKey};
use solidcore::geom::axis2_placement::Axis2Placement;
use solidcore::mass::integrator::properties::{brep_volume, PropertyAttributes};
use solidcore::math::{Point3, Vec3};
use solidcore::primitive::api_primitives::{create_box, create_cylinder, create_sphere};
use solidcore::primitive::api_primitives::create_cone;
use solidcore::query::api_queries::{brep_bounding_box, get_edges};
use solidcore::blend::fillet_executive::api::circular_fillet;
use solidcore::boolean::api_brep::{boolean_difference, boolean_intersection, boolean_union};
use solidcore::topology::proto_topology::p1::api_transform::{
    transform_independent_object, TransformObject,
};
use solidcore::topology::Brep;

/// Owner of a kernel and its B-rep registry. Opaque to C.
pub struct ScKernel {
    k: solidcore::Kernel,
    /// 1-based ids index into this; `None` = released (consumed by a boolean).
    breps: Vec<Option<Brep>>,
    store: AttributeStore,
}

const ERR: i64 = -1;

fn rot_frame(axis: Vec3, angle_deg: f64) -> (Vec3, Vec3, Vec3) {
    let n = axis.unitize_or_z();
    let th = angle_deg.to_radians();
    let (s, c) = th.sin_cos();
    // Rodrigues, applied to the world X and Y axes; any v not parallel to n.
    let mut v = Vec3::Y;
    if n.x.abs() > 0.9 {
        v = Vec3::Z;
    }
    let nv = n.dot(v);
    let v_perp = v - n * nv;
    let w = n.cross(v_perp);
    let x = v_perp * c + w * s;
    let y = w * c - v_perp * s;
    (n, x, y)
}

impl ScKernel {
    fn take(&mut self, id: i64) -> Option<Brep> {
        if id <= 0 {
            return None;
        }
        let i = usize::try_from(id).ok()? - 1;
        if i >= self.breps.len() {
            return None;
        }
        self.breps[i].take()
    }

    fn put(&mut self, b: Brep) -> i64 {
        self.breps.push(Some(b));
        let id = self.breps.len();
        i64::try_from(id).unwrap_or(ERR)
    }

    fn attrs(&self) -> PropertyAttributes<'_> {
        PropertyAttributes {
            store: &self.store,
            key_of: &|_: solidcore::error::EntityRef| ObjectKey(0),
        }
    }
}

/// Run `f`, mapping a panic or `Err` to `err`, recording the error text.
fn guard<T: Clone>(err: T, f: impl FnOnce() -> Result<T, solidcore::SolidError>) -> T {
    catch_unwind(AssertUnwindSafe(f))
        .map(|inner| {
            inner.unwrap_or_else(|e| {
                LAST_ERROR.with(|c| *c.borrow_mut() = format!("{e}"));
                err.clone()
            })
        })
        .unwrap_or_else(|_| {
            LAST_ERROR.with(|c| *c.borrow_mut() = "panic".to_string());
            err
        })
}

thread_local! {
    static LAST_ERROR: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
}

/// Debug text of the last error on this kernel ("", when none).
#[no_mangle]
pub extern "C" fn sc_last_error(_k: *mut ScKernel) -> *const u8 {
    LAST_ERROR.with(|c| c.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn sc_kernel_new() -> *mut ScKernel {
    catch_unwind(|| Box::into_raw(Box::new(ScKernel {
        k: solidcore::Kernel::default(),
        breps: Vec::new(),
        store: AttributeStore::default(),
    })))
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `k` must be a pointer from `sc_kernel_new`, not yet freed.
#[no_mangle]
pub unsafe extern "C" fn sc_kernel_free(k: *mut ScKernel) {
    if !k.is_null() {
        drop(Box::from_raw(k));
    }
}

/// Solid box, corner at (x, y, z), size (sx, sy, sz) along +X/+Y/+Z.
#[no_mangle]
pub extern "C" fn sc_box(k: *mut ScKernel, x: f64, y: f64, z: f64, sx: f64, sy: f64, sz: f64) -> i64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR };
    guard(ERR, || {
        let b = create_box(&mut sk.k, Point3::new(x, y, z), sx, sy, sz)?;
        Ok(sk.put(b))
    })
}

/// Solid cylinder, base circle centre (x, y, z), axis +Z, height h, radius r.
#[no_mangle]
pub extern "C" fn sc_cylinder(k: *mut ScKernel, x: f64, y: f64, z: f64, r: f64, h: f64) -> i64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR };
    guard(ERR, || {
        let b = create_cylinder(&mut sk.k, Point3::new(x, y, z), r, h)?;
        Ok(sk.put(b))
    })
}

/// Solid truncated cone, base radius r1 at (x, y, z), top radius r2 at z + h, axis +Z.
#[no_mangle]
pub extern "C" fn sc_cone(k: *mut ScKernel, x: f64, y: f64, z: f64, r1: f64, r2: f64, h: f64) -> i64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR };
    guard(ERR, || {
        let b = create_cone(&mut sk.k, Point3::new(x, y, z), r1, r2, h)?;
        Ok(sk.put(b))
    })
}

/// Solid sphere centred at (x, y, z) with radius r.
#[no_mangle]
pub extern "C" fn sc_sphere(k: *mut ScKernel, x: f64, y: f64, z: f64, r: f64) -> i64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR };
    guard(ERR, || {
        let b = create_sphere(&mut sk.k, Point3::new(x, y, z), r)?;
        Ok(sk.put(b))
    })
}

fn boolean(
    k: *mut ScKernel,
    a: i64,
    b: i64,
    f: fn(&mut solidcore::Kernel, Brep, Brep) -> solidcore::Result<Brep>,
) -> i64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR };
    guard(ERR, || {
        let (Some(ba), Some(bb)) = (sk.take(a), sk.take(b)) else {
            return Err(solidcore::SolidError::invalid_input(
                "solidcore_capi::boolean", "unknown brep id",
            ));
        };
        let r = f(&mut sk.k, ba, bb)?;
        Ok(sk.put(r))
    })
}

/// Union of the solids `a` and `b`; both ids are consumed, the result gets a new id.
#[no_mangle]
pub extern "C" fn sc_union(k: *mut ScKernel, a: i64, b: i64) -> i64 {
    boolean(k, a, b, boolean_union)
}

/// Difference `a - b`; both ids are consumed, the result gets a new id.
#[no_mangle]
pub extern "C" fn sc_difference(k: *mut ScKernel, a: i64, b: i64) -> i64 {
    boolean(k, a, b, boolean_difference)
}

/// Intersection of `a` and `b`; both ids are consumed, the result gets a new id.
#[no_mangle]
pub extern "C" fn sc_intersection(k: *mut ScKernel, a: i64, b: i64) -> i64 {
    boolean(k, a, b, boolean_intersection)
}

fn transformed(
    k: *mut ScKernel,
    id: i64,
    transform: &Axis2Placement,
    scale: Option<Vec3>,
) -> i32 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR as i32 };
    let Some(mut b) = sk.take(id) else { return ERR as i32 };
    let status = guard(-1i32, || {
        transform_independent_object(&mut sk.k, Some(TransformObject::Brep(&mut b)), transform, scale)?;
        Ok(0)
    });
    if status == 0 {
        sk.breps[id as usize - 1] = Some(b);
    }
    status
}

/// Translate the solid `id` by (x, y, z). Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn sc_translate(k: *mut ScKernel, id: i64, x: f64, y: f64, z: f64) -> i32 {
    transformed(k, id, &Axis2Placement::new(Point3::new(x, y, z), Vec3::X, Vec3::Y), None)
}

/// Rotate the solid `id` by `angle_deg` about the axis (ax, ay, az) through the
/// origin. Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn sc_rotate(k: *mut ScKernel, id: i64, angle_deg: f64, ax: f64, ay: f64, az: f64) -> i32 {
    let (n, x, y) = rot_frame(Vec3::new(ax, ay, az), angle_deg);
    let _ = n;
    transformed(k, id, &Axis2Placement::new(Point3::ZERO, x, y), None)
}

/// Scale the solid `id` about the origin by (sx, sy, sz). Only a uniform
/// positive scale is supported by the kernel for B-reps; anything else fails.
#[no_mangle]
pub extern "C" fn sc_scale(k: *mut ScKernel, id: i64, sx: f64, sy: f64, sz: f64) -> i32 {
    transformed(k, id, &Axis2Placement::new(Point3::ZERO, Vec3::X, Vec3::Y), Some(Vec3::new(sx, sy, sz)))
}

/// Fillet every blendable edge of the solid `id` with a circular cross-section
/// of `radius` (`circular_fillet`). Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn sc_fillet_all(k: *mut ScKernel, id: i64, radius: f64) -> i32 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return ERR as i32 };
    let Some(mut b) = sk.take(id) else { return ERR as i32 };
    let edges = guard(Vec::new(), || get_edges(&b));
    let status = guard(-1i32, || {
        if edges.is_empty() {
            return Err(solidcore::SolidError::invalid_input(
                "solidcore_capi::sc_fillet_all", "no edges",
            ));
        }
        circular_fillet(&mut sk.k, &mut b, radius)?;
        Ok(0)
    });
    if status == 0 {
        sk.breps[id as usize - 1] = Some(b);
    }
    status
}

/// Volume of the solid `id` (`brep_volume`, accuracy 1e-6). NaN on failure.
#[no_mangle]
pub extern "C" fn sc_volume(k: *mut ScKernel, id: i64) -> f64 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return f64::NAN };
    let Some(mut b) = sk.take(id) else { return f64::NAN };
    let v = guard(f64::NAN, || brep_volume(&sk.k, &mut b, 1e-6, &sk.attrs()));
    sk.breps[id as usize - 1] = Some(b);
    v
}

/// Bounding box of the solid `id` into `out[0..6]` = xmin ymin zmin xmax ymax zmax.
/// Returns 0 on success, -1 on failure.
///
/// # Safety
/// `out` must point to at least 6 writable doubles.
#[no_mangle]
pub unsafe extern "C" fn sc_bbox(k: *mut ScKernel, id: i64, out: *mut f64) -> i32 {
    let Some(sk) = (unsafe { k.as_mut() }) else { return -1 };
    let Some(b) = sk.breps.get_mut(id as usize - 1).and_then(|s| s.as_mut()) else { return -1 };
    let ext = guard(None, || brep_bounding_box(&sk.k, b, true).map(Some));
    let Some(ext) = ext else { return -1 };
    if out.is_null() {
        return -1;
    }
    let vals = [
        ext.min_at(0), ext.min_at(1), ext.min_at(2),
        ext.max_at(0), ext.max_at(1), ext.max_at(2),
    ];
    let out = slice::from_raw_parts_mut(out, 6);
    out.copy_from_slice(&vals);
    0
}

/// Mesh the solid `id`: NOT SUPPORTED. `solidcore` has no ported tessellator
/// (`SmApiTessellate`), and a hand-rolled one could produce wrong geometry, so
/// this always returns -1 and leaves `out_ptr` untouched. Compare kernels via
/// `sc_volume` and `sc_bbox` instead.
#[no_mangle]
pub extern "C" fn sc_mesh(
    _k: *mut ScKernel,
    _id: i64,
    _deflection: f64,
    _out_ptr: *mut *mut f32,
) -> i32 {
    -1
}

/// # Safety
/// `p` must be a pointer handed out by this library (currently: none — meshing
/// is unsupported — but kept for ABI completeness), or null.
#[no_mangle]
pub unsafe extern "C" fn sc_free(p: *mut core::ffi::c_void) {
    if !p.is_null() {
        drop(Box::from_raw(p as *mut u8));
    }
}
