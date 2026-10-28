# solidcore-capi

A minimal C ABI over the [`solidcore`](../solidcore) B-rep kernel (the Rust port
of NVIDIA's SMLib), built as a cdylib: `target/release/libsolidcore_capi.so`.

## Build

```sh
cargo build -p solidcore-capi --release
```

The crate is a workspace member but opts out of the workspace lints: FFI glue
needs `unsafe` and small unwraps at the panic boundary.

## Model

`sc_kernel_new()` returns an opaque `ScKernel*` that owns a `solidcore::Kernel`
(tolerances, deterministic RNG, caches) and a registry of B-reps addressed by
**1-based ids**. Every function taking an id looks the B-rep up in the owning
kernel's registry; there is no global state. Booleans consume their operands:
`sc_union(k, a, b)` releases both ids and returns a fresh one for the result.

Every entry point catches panics at the boundary (`catch_unwind`) and returns
an error code; nothing unwinds into C.

## Functions

| Function | Semantics | Errors |
| --- | --- | --- |
| `sc_kernel_new` / `sc_kernel_free` | create/destroy a kernel | null / — |
| `sc_box(k,x,y,z,sx,sy,sz)` | solid box, **corner** at `(x,y,z)`, sizes along +X/+Y/+Z | id `-1` |
| `sc_cylinder(k,x,y,z,r,h)` | solid cylinder, base circle centre `(x,y,z)`, axis +Z | id `-1` |
| `sc_cone(k,x,y,z,r1,r2,h)` | truncated cone, base `r1` at z, top `r2` at z+h | id `-1` |
| `sc_last_error(k)` | debug text of the last error (valid until the next call) | — |
| `sc_sphere(k,x,y,z,r)` | solid sphere centred at `(x,y,z)` | id `-1` |
| `sc_union` / `sc_difference` / `sc_intersection` | boolean, operands consumed | id `-1` |
| `sc_translate(k,id,x,y,z)` | move the solid | 0 ok / `-1` |
| `sc_rotate(k,id,deg,ax,ay,az)` | rotate about the axis through the origin | 0 ok / `-1` |
| `sc_scale(k,id,sx,sy,sz)` | scale about the origin (B-reps take uniform positive scales) | 0 ok / `-1` |
| `sc_fillet_all(k,id,r)` | `circular_fillet`: every blendable edge, circular cross-section | 0 ok / `-1` |
| `sc_volume(k,id)` | `brep_volume`, accuracy 1e-6 | NaN |
| `sc_bbox(k,id,out)` | `out[0..5]` = xmin ymin zmin xmax ymax zmax | 0 ok / `-1` |
| `sc_mesh(k,id,deflection,out_ptr)` | **NOT SUPPORTED** (always `-1`) | — |
| `sc_free(p)` | free memory handed out by this library | — |

### No tessellation

`sc_mesh` always returns `-1`: SMLib's `SmApiTessellate` has not been ported
and hand-rolling a tessellator risks wrong geometry. Compare kernels through
`sc_volume` and `sc_bbox` instead (that is what openscad-occt's
`tests/compare-kernels.sh` does for this backend, via `oscad --metrics`).

### Fillet selection

`sc_fillet_all` rounds every edge that bounds a blendable sector. There is no
convex/concave classification in this ABI; a caller needing it must filter
elsewhere or wait for an `sc_fillet_edges` extension.
