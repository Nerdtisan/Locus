## Fixtures (closed areas, round joins)

Cells: verdict · arcs/lines · loops · time per offset.

| Fixture | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| rect 40×20, d = +1 | pass · 4/4 · 1 loops · 0.07 ms | pass · 4/4 · 1 loops · 0.00 ms |
| rect 40×20, d = -1 | pass · 0/4 · 1 loops · 0.03 ms | pass · 0/4 · 1 loops · 0.00 ms |
| rect 40×20, d = -10 | pass · 0/0 · 0 loops · 0.03 ms | **FAIL** (1 sliver) · 0/2 · 1 loops · 0.00 ms |
| star, 12 points (tight inside corners), d = +1 | pass · 12/24 · 1 loops · 0.63 ms | pass · 12/24 · 1 loops · 0.01 ms |
| star, 12 points (tight inside corners), d = -1 | pass · 12/24 · 1 loops · 0.63 ms | pass · 12/24 · 1 loops · 0.01 ms |
| star, 12 points (tight inside corners), d = -3 | pass · 12/0 · 1 loops · 1.32 ms | pass · 12/0 · 1 loops · 0.01 ms |
| comb, 0.3 mm notches, d = +0.15 | pass · 22/13 · 1 loops · 0.55 ms | pass · 22/31 · 1 loops · 0.01 ms |
| comb, 0.3 mm notches, d = +0.2 | pass · 22/13 · 1 loops · 0.83 ms | pass · 22/13 · 1 loops · 0.01 ms |
| comb, 0.3 mm notches, d = -0.2 | pass · 18/40 · 1 loops · 0.81 ms | pass · 18/40 · 1 loops · 0.01 ms |
| rounded rect, r 0.5 (small radii), d = +1 | pass · 4/4 · 1 loops · 0.06 ms | pass · 4/4 · 1 loops · 0.00 ms |
| rounded rect, r 0.5 (small radii), d = -0.5 | pass · 0/4 · 1 loops · 0.05 ms | pass · 0/4 · 1 loops · 0.00 ms |
| rounded rect, r 0.5 (small radii), d = -1 | pass · 0/4 · 1 loops · 0.06 ms | pass · 0/4 · 1 loops · 0.00 ms |
| circle r 1, d = -0.999 | pass · 1/0 · 1 loops · 0.01 ms | pass · 2/0 · 1 loops · 0.00 ms |
| circle r 1, d = -1 | pass · 0/0 · 0 loops · 0.01 ms | pass · 0/0 · 0 loops · 0.00 ms |
| circle r 1, d = +2 | pass · 1/0 · 1 loops · 0.01 ms | pass · 2/0 · 1 loops · 0.00 ms |
| slot r 3, d = +2 | pass · 2/2 · 1 loops · 0.03 ms | pass · 2/2 · 1 loops · 0.00 ms |
| slot r 3, d = -2.9 | pass · 2/2 · 1 loops · 0.03 ms | pass · 2/2 · 1 loops · 0.00 ms |
| slot r 3, d = -3 | pass · 0/0 · 0 loops · 0.02 ms | **FAIL** (1 sliver) · 0/2 · 1 loops · 0.00 ms |
| nested: plate, hole, island, slot hole, d = +0.1 | pass · 7/10 · 4 loops · 0.14 ms | pass · 8/10 · 4 loops · 0.00 ms |
| nested: plate, hole, island, slot hole, d = -0.1 | pass · 11/10 · 4 loops · 0.19 ms | pass · 12/10 · 4 loops · 0.00 ms |
| nested: plate, hole, island, slot hole, d = +5 | pass · 5/8 · 3 loops · 0.18 ms | pass · 8/10 · 2 loops · 0.00 ms |
| nested: plate, hole, island, slot hole, d = +6 | pass · 6/10 · 3 loops · 0.20 ms | pass · 8/10 · 3 loops · 0.00 ms |
| gear, 60 teeth, d = +0.5 | pass · 60/0 · 1 loops · 3.87 ms | pass · 120/0 · 1 loops · 0.06 ms |
| gear, 60 teeth, d = -0.5 | pass · 240/0 · 1 loops · 10.87 ms | pass · 300/0 · 1 loops · 0.05 ms |
| gear, 60 teeth, d = -1 | pass · 240/0 · 1 loops · 12.52 ms | pass · 300/0 · 1 loops · 0.05 ms |
| plate − 50 holes, d = +0.1 | pass · 54/4 · 51 loops · 0.47 ms | pass · 104/4 · 51 loops · 0.06 ms |
| plate − 50 holes, d = -0.1 | pass · 54/4 · 51 loops · 0.51 ms | pass · 104/4 · 51 loops · 0.06 ms |
| gear, 240 teeth (480 arcs), d = +0.5 | pass · 240/0 · 1 loops · 23.83 ms | pass · 480/0 · 1 loops · 0.33 ms |
| gear, 240 teeth (480 arcs), d = -0.5 | pass · 960/0 · 1 loops · 68.79 ms | pass · 1200/0 · 1 loops · 0.23 ms |

## Fixtures (open chains, one side)

| Fixture | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| zigzag (sharp turns), left 1 | pass · 1/4 · 1 runs · 0.06 ms | pass · 1/4 · 1 runs · 0.00 ms |
| zigzag (sharp turns), right 1 | pass · 2/4 · 1 runs · 0.06 ms | pass · 2/4 · 1 runs · 0.00 ms |
| J hook, r 1 arc, inside 2 (arc collapses) | pass · 1/2 · 1 runs · 0.02 ms | pass · 1/2 · 1 runs · 0.00 ms |
| J hook, outside 2 | pass · 0/1 · 1 runs · 0.02 ms | pass · 0/1 · 1 runs · 0.00 ms |
| S-wave of arcs, left 1 | pass · 6/0 · 1 runs · 0.04 ms | pass · 6/0 · 1 runs · 0.00 ms |
| S-wave of arcs, right 2.5 | pass · 3/0 · 1 runs · 0.06 ms | pass · 3/0 · 1 runs · 0.00 ms |

## Randomised sweeps, 2000 cases each

| Sweep | A: Locus-owned | B: cavalier_contours |
|---|---|---|
| 0.5 mm grid, d in 0.25 steps (exact touching / collapse) | 2000 pass / 0 fail / 0 panic · worst failing dev 0.0e0 · mean 0.27 ms | 1970 pass / 30 fail / 0 panic · worst failing dev 1.5e0 · mean 0.01 ms · first fail #105 |
| grid shapes, d in 0.25 steps (second seed) | 1999 pass / 1 fail / 0 panic · worst failing dev 1.8e-15 · mean 0.28 ms · first fail #1752 | 1968 pass / 32 fail / 0 panic · worst failing dev 1.2e0 · mean 0.01 ms · first fail #95 |
| general position (random floats) | 2000 pass / 0 fail / 0 panic · worst failing dev 0.0e0 · mean 0.27 ms | 2000 pass / 0 fail / 0 panic · worst failing dev 0.0e0 · mean 0.01 ms |
