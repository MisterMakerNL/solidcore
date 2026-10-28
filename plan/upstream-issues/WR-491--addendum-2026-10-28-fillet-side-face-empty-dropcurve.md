#### Updates: WR-491 (2026-10-28). The empty-DropCurve corner edge ported as an opt-in fix

- **Fix field:** `Kernel::fixes.fillet_side_face_empty_dropcurve`
  (`UpstreamFixes`, off by default;
  `crates/solidcore/src/kernel.rs`). Off, the port matches the pinned C++
  bit-for-bit: the corner edge stays `RailRailInterpolation` and the fillet fails
  with the generic upstream status 1001.
- **Rust port:** in `FilletBrep::calc_rail_rail_interpolation`
  (`crates/solidcore/src/blend/fillet_geom/p2.rs`, upstream
  `SmFilletEdge::CalcRailRailInterpolation`, `SmFilletGeom.cpp:6666..7024`), when
  the `DropCurve` of the corner edge's interpolated 3-D curve onto the corner's
  original face succeeds but yields **zero** UV curves at a corner edge
  (`opt_corner.is_some()`), the failing edge's fillet type is set to
  `FilletEdgeType::FilletXSideFace` before the `SM_ERR` is raised. The caller
  (`FilletCorner::calc_corner_edge_geom_with`, the port of
  `SmFilletCorner::CalcCornerEdgeGeom`, `SmFilletCorner.cpp:1633-1641`) already
  deletes a failed fillet-side-face edge when the executive does a global merge
  (`SmApiCircularFillet` always enables it), and the merge stage closes the gap
  the deleted corner edge leaves — the path the C++ exemption would have taken
  had the corner been classified `SM_FE_FILLET_X_SIDE_FACE` in the first place.
  The `sUVCurves.GetSize() == 1` guard is otherwise untouched: a drop-curve that
  returns more than one UV curve still fails as upstream, and the second
  (reprojection after `LiftCurve`) guard is untouched.
- **Kind:** a result-changing fix (the fillet completes instead of refusing), so
  it follows the opt-in rule; off by default the port matches the pinned C++.
- **Test:** `crates/solidcore/src/blend/fillet_geom/p2.rs`: box 100x100x10 unioned
  with a cylinder r=20 h=20 based at `(100, 50, 10)` (axis on the plate's edge),
  `circular_fillet(r=2, all edges)`.
  `the_cliff_end_fillet_refuses_without_the_side_face_fix` asserts the flag-off
  behaviour (upstream status 1001). The flag-on completion test
  (`the_cliff_end_fillet_completes_with_the_side_face_fix`, volume against OCCT
  7.6.3's 124299) is `#[ignore]`d: the Rust port fails **earlier** than the C++
  chain on this input, at `calc_rail_int_rail`'s rail-rail vertex solve, before
  the corner edge's DropCurve the fix repairs — recorded in
  `plan/findings/blend.fillet_geom_p2--plate-cylinder-cliff-end-rail-rail-vertex-divergence-1410.md`.
- **Not covered:** the same empty-DropCurve condition reached through the
  function's non-corner path (`opt_corner = None`) or through the reprojection
  guard after `LiftCurve`; both keep upstream behaviour. End-to-end proof on the
  issue's case waits on the rail-rail vertex divergence above.
