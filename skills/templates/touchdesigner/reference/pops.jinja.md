# POPs (Point Operators) — deep dive

Source: [Learning About POPs](https://derivative.ca/UserGuide/Learning_About_POPs),
[2025.31550 release notes](https://derivative.ca/release/202531550/73152),
[Attribute](https://docs.derivative.ca/Attribute). New family in TD 2025; GPU-resident
3D data. In TD, run **Help → OP Snippets** for 1000+ live POP examples.

Live-verified on build 2025.33070: 101 POP types; a fresh `circlePOP` carries point
attributes `P` (3), `N` (3), `Tex` (3) — and no `Color`, matching the wiki's note that
generators don't add it.

## Data model

A POP = **point list + vertex list + primitive list**, each carrying named attributes.

- **Points**: the data rows. Standard attributes: `P` (float3 position — optional!),
  `N` (float3 normal), `Color` (always float4 rgba), `Tex` (always float3 texcoords).
  Common conventional names: `Weight`, `PointScale`, `LineWidth`, `Vel`, `Dir`, `Dist`,
  `Index`; particle attrs get a `Part` prefix (`PartVel`, `PartId`), Ray POP a `Ray` prefix.
- **Primitives**: Point (1), Line (2), Line Strip (n), Triangle (3), Quad (4). Stored
  regrouped: triangles first, then quads, line strips, lines, points.
- **Vertices**: indices into the point list, per primitive; can carry their own
  attributes (use vertex attrs when a shared point needs per-face values like seams
  in texture coords).
- Attribute types: float/double/int/uint; sizes: scalar, vec2-4, matrices up to 4x4,
  plus array attributes (`MyArray[4]` `(2)` addressing).
- **Swizzling**: `P(0) P(1) P(2)` == `P.xyz`; `Color.rgb`, `Color.a`; reorder with
  `P.yxz` or `P(1,0,2)` — component reorder on input/output scope is a first-class
  mixing tool.
- Custom attribute names: letters only, start lower-case by convention (capitalized
  initial is reserved for Derivative-defined names).

## Key mechanics

- **Points don't render without primitives.** A point cloud renders only if points have
  Point primitives (generators create them with Connectivity = Point Primitives; the
  Primitive POP adds/strips them). Instancing, by contrast, reads the raw point list.
- **Memory via references**: a POP allocates only the attributes it changes; the rest
  pass through as references (`(r)` in the middle-click info popup, and the viewer's
  bottom-left corner lists created/modified attrs). This is also the debugging view for
  "which POP actually touched my data".
- **Read-only built-ins** usable in Math/Lookup/Delete/Group POPs (never output):
  `_PointI` / `_PointU` / `_PointCy` (index, 0-1 normalized, cyclic), `_PrimI`,
  `_VertI` variants, `_DimI[]` (grid row/col/slice), `_StepSeconds`.
- **index vs id**: index is invariant list position (not an attribute, not writable);
  `id`-style attributes (`PartId`) stick to an entity for its lifetime and may be sparse.
- **Groups**: bit-flag sets (max 32/POP) made by the Group POP; most filter POPs accept
  a group scope. **Dimension** is structured-grid metadata (rows/cols/slices) that flows
  between POPs.
- **Feedback**: particle-style simulation = integrate over time via Particle POP, or a
  Feedback POP loop; forces accumulate in `PartForce`.

## Workhorse POPs

Generators: Point Generator, Circle, Box, Sphere, Grid, Line, Point File In (clouds),
File In (FBX/obj/Alembic). Filters: Transform, Math, Math Mix, Noise, Pattern, Normalize,
ReRange, Limit, Lookup Texture/Channel/Attribute, Attribute (create), Attribute Combine,
Attribute Convert (point↔vertex↔prim class), Convert (prim types), Copy, Merge, Delete,
Sort, Group, Line Break/Divide/Smooth/Resample, Trail (point history → line strips),
Proximity, Neighbor. Custom compute: GLSL POP / GLSL Advanced POP / Copy GLSL POP.

## Interchange & output

- TOP ↔ POP: `TOP to POP` (e.g. depth/point-cloud textures → points) and `POP to TOP`;
  stays on GPU.
- POP → CHOP (`POP to CHOP`) forces GPU→CPU copies — do math in POPs (Math/Math Mix)
  instead when possible; that's the family's design goal.
- `POP to DAT` (Extract: Points/Vertices/Primitives) is the inspection table — the
  first thing to wire when a POP chain misbehaves.
- Rendering: put POPs inside a Geometry COMP, apply a MAT, render with Render TOP.
  DMX POPs output directly to DMX/Art-Net/sACN fixtures.
- Python (CPU side, use `delayed=True` to avoid stalls):
  `op('pop1').pointAttributes` (iterate for `.name/.size/.type`),
  `pointAttributesChanged`, `op('pop1').points('Attr')`, `op('pop1').dimension`.

## Current gaps (vs SOPs)

No Mesh primitive (rows/cols grids — POP "dimensions" covers most uses), no surface
booleans, no NURBS. Keep SOPs for those; convert at the boundary.


---

**Canonical:** {{ skill("pops") }} 
