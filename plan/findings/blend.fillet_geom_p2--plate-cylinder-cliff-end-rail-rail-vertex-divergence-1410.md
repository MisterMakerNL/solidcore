# blend.fillet_geom_p2: the Rust port fails the #1410 cliff-end fillet earlier than the C++ — a rail-rail vertex solve that never converges (issue #1410, 2026-10-28)

Unit: `blend.fillet_geom_p2` (`FilletBrep::calc_rail_int_rail`, upstream
`SmFilletVertex::CalcRailIntRail`), reached from `blend.fillet_executive`'s
`create_corner_vertices` (`SmFilletExecutive::CreateCornerVertices`), i.e. the corner
**vertex** geometry stage, before the corner **edge** stage
(`CalcCornerEdgeGeom`) where the WR-491 defect sits
(`plan/upstream-issues/WR-491--circular-fillet-refuses-edge-ending-at-free-face.md`).

Inputs: box 100x100x10 unioned with a cylinder r=20 h=20 based at `(100, 50, 10)` (axis
on the plate's edge), `circular_fillet(r=2, all edges)`.

## What the pinned C++ does (per the WR-491 record)

`CreateCornerVertices` succeeds on every corner; the failure is later, in the corner-edge
loop: the free-end corner's `SM_FE_RAIL_RAIL_INTERPOLATION` edge fails because `DropCurve`
yields zero UV curves (`SmFilletGeom.cpp:7024`).

## What the Rust port does (debug-instrumented run, instrumentation removed after)

The port fails in `create_corner_vertices`, at corner `FilletCornerId(9)`: its fillet
vertex `VertexId(32)` is classified `RailXRail`, and
`FilletBrep::calc_rail_int_rail` reports the rail-rail solve as **not converged**:

```
DBG rir face=Some(FaceId(8)) r1=1 r2=1 s1=1 s2=0 uv0=(27.136, 0.0) uv1=(35.696, 0.0)
DBG rir not converged t1=[0,0,0,0] t2=[0,0,0,0]
DBG live vert VertexId(32) RailXRail: calc_rail_int_rail: upstream status 1001
DBG calc_vert FilletCornerId(9): calc_corner_vert_geom: upstream status 1001
DBG create_corner_vertices: blend::fillet_executive::p3: upstream status 1001
```

Both rails are read (`rail_index1 = rail_index2 = 1` on the common face), the guess UVs
are computed from each filleted edge (2 radii along each), and
`rail_rail_intersect` returns `false` without moving the TsectPnts. The two rails sit
about 8.5 apart along the face's first parameter, so at face value they do not meet — in
the C++ the corresponding vertex computes, which means either the port's rails/traces at
this corner differ (built earlier, in the solver/trace stage) or its rail-rail vertex
solve diverges. Both flags off and on fail identically here; no existing opt-in
`fixes.fillet_*` flag changes the outcome (checked one by one against this body).

## Status

This divergence **blocks the end-to-end validation** of the WR-491 opt-in fix
(`fixes.fillet_side_face_empty_dropcurve`, see its 2026-10-28 addendum): the corner-edge
path the fix repairs is never reached on this input. The fix itself stays in
(guarded, off by default, default behaviour bit-for-bit unchanged) and its regression
test's flag-on half is `#[ignore]`d until this is resolved. Unit stays as it is; the
candidate follow-ups are the solver/trace stage that builds the two rails at this
vertex (compare against the C++ trace ends on the same input) and the rail-rail
vertex solve's iteration/retry behaviour.
