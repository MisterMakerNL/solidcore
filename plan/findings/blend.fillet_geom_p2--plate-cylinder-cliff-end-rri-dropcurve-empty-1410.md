# blend.fillet_geom_p2: plate-cylinder cliff-end corner — rail-rail interpolation drop-curve yields zero UV curves (issue #1410, 2026-10-28)

Unit: `blend.fillet_geom_p2` (`FilletBrep::calc_rail_rail_interpolation`, upstream
`SmFilletEdge::CalcRailRailInterpolation`), reached from `blend.fillet_corner::calc_corner_edge_geom`
and `blend.fillet_executive::session::finish::create_fillet_brep`.

Inputs: box 100x100x10 unioned with a cylinder r=20 h=20 whose axis lies on the plate edge
(x=100, half the cylinder overhangs); `SmApiCircularFillet` / `circular_fillet(r=2, all edges)`.

Result: both the pinned C++ (v1.0.1, 3fd5b95) and the Rust port fail with status 1001
(`blend::fillet_executive::session::finish: upstream status 1001`); OCCT 7.6.3 rounds the
same solid (volume ~124299 mm3). Radius-independent (r=0.5..3 all fail).

## C++ failure chain (instrumented driver, sources reverted after)

1. `SmApiCircularFillet` (`SM_API/src/SmApiFillets.cpp:172`) -> `CreateFilletCorners` OK ->
   `DoFilleting` (`SmFilletExecutive.cpp:2957`) -> `CreateFilletBrep` (`:1931`).
2. `CreateFilletBrep` corner loop, second pass (`SmFilletExecutive.cpp:2359-2368`):
   `pCorner->CalcCornerEdgeGeom()` fails for the corner at the **free end of the concave
   junction edge** (where the junction arc terminates on the plate's side face) ->
   `FILEXEC_CORNER_ERR` -> status 1001. The other, on-face corner passes.
3. Inside the corner (`SmFilletCorner.cpp:1609`), the failing edge has type
   `SM_FE_RAIL_RAIL_INTERPOLATION` (enum value 11; `SM_FE_*` enum starts at
   `SM_FE_BLENDING_RAIL=0`), so `SmFilletEdge::CalcRailRailInterpolation`
   (`SmFilletGeom.cpp:6666`) runs and fails.
4. Exact branch: the interpolated rail-to-rail 3-D curve is projected onto the corner's
   original face with `pSurf->DropCurve(...)` (`SmFilletGeom.cpp:~6997`); `DropCurve`
   **succeeds but returns an empty** `sUVCurves` array, and the
   `sUVCurves.GetSize() == 1` guard raises `SER(SM_ERR)` at `SmFilletGeom.cpp:7024`.
5. Because the edge type is `SM_FE_RAIL_RAIL_INTERPOLATION` and not
   `SM_FE_FILLET_X_SIDE_FACE`, the global-merge forgiveness at
   `SmFilletCorner.cpp:1633-1641` does not apply (it would just delete the edge and let
   the merge stage resolve the gap) — `SER(SM_ERR)` at `SmFilletCorner.cpp:1637` aborts.

Instrumentation evidence (debug prints added to `SmFilletGeom.cpp`/`SmFilletCorner.cpp`,
incremental rebuild of the openscad-occt SMLib target, then reverted):

```
DBG RRI ENTER
DBG RRI before dropcurve1
DBG RRI FAIL dropcurve1 count=0 maxdist=0
DBG corner edge FAILED type=11 globalMerge=1
```

## Rust sites (same chain, ported 1:1)

- `crates/solidcore/src/blend/fillet_geom/p2.rs:2544-2548` — `surface_drop_curve(...)`
  (line 2544) returns an empty `uv_curves` vec; `if uv_curves.len() != 1 { return
  Err(SolidError::sm_err(OP)) }` (line 2546) with
  `OP = "blend::fillet_geom::p2::FilletBrep::calc_rail_rail_interpolation"` (line 136).
- `crates/solidcore/src/blend/fillet_corner/p1.rs:1613-1624` — `calc_corner_edge_geom_with`:
  calc error + `edge_type != FilletEdgeType::FilletXSideFace` -> `Err(SolidError::sm_err(OP_EDGE_GEOM))`.
- `crates/solidcore/src/blend/fillet_executive/session/finish.rs:475-481` —
  `create_fillet_brep` failure -> `Err(failure())` = `SolidError::Upstream { code: 1001 }`
  (`OP = "blend::fillet_executive::session::finish"`, line 38).

## Why OCCT succeeds

OCCT's `BRepFilletAPI` treats a blend whose spine runs off a free face as a normal
"blend ending on a face": it extends/trims the fillet surface onto that face (the
cliff-overflow family, solidcore issues #960/#1071/#1073/#1074). SMLib's corner machinery
does have cliff machinery (`CreateCliffRollOverSideFG`, `CalcCliffRailIntEdge`), but for
this geometry the corner is classified as a rail-rail interpolation case, and the
interpolated cross-section curve cannot be dropped onto the corner face at all — the
projection is empty rather than merely inaccurate, so no tolerance relaxation helps
(matching the issue's offset-independence).

## Minimal fix sketch

The failing branch is the strict `uv_curves.len() == 1` guard after `DropCurve`. Options:

1. Forgiving path (smallest): when the drop-curve is empty at a corner whose vertex lies
   on a free face, reclassify the edge as `SM_FE_FILLET_X_SIDE_FACE` (or return a
   controlled failure the caller already handles) so `SmFilletCorner::CalcCornerEdgeGeom`'s
   existing global-merge forgiveness deletes the edge and the merge stage closes the gap.
   Port side: mirror in `fillet_corner::p1::calc_corner_edge_geom_with` (its
   `FilletXSideFace` carve-out already exists) and gate behind `Kernel::fixes` per the
   repo rules.
2. Constructive path: build the UV curve directly in the face's UV space (the
   `!bCreateCrvOn3D` branch already interpolates in UV) instead of requiring a successful
   3-D projection; more work, better geometry.

Both need a regression fixture (the `.scad` reproducer of issue #1410) and a difftest
entry; option 1 is roughly a day of work plus gate time, option 2 is several days.

PR: claim PR `fix: blend cliff-end rail-rail interpolation — root cause: DropCurve yields 0 UV curves at the free-face corner (claim, #1410)`.
