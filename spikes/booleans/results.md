
### Fixtures

| Fixture | Option | Pass | Dev out→ref (mm) | Dev ref→out (mm) | ΔArea (mm²) | Arcs / lines (expected) | Loops | µs |
|---|---|---|---|---|---|---|---|---|
| rect ∪ circle on corner | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 1.6e-6 | 1 / 4 (1 / 4) | 1 | 1442 |
| rect ∪ circle on corner | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 7.8e-3 | 0 / 172 (1 / 4) | 1 | 20 |
| rect ∪ circle on corner | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 1.6e-6 | 2 / 4 (1 / 4) | 1 | 1 |
| rect ∪ circle on corner | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 1.6e-6 | 1 / 4 (1 / 4) | 1 | 6 |
| slot − circle over cap | A: i_overlay + refit | yes | 2.0e-4 | 2.0e-4 | 5.7e-4 | 3 / 2 (?) | 1 | 1347 |
| slot − circle over cap | A0: i_overlay lines | yes | 5.0e-4 | 6.1e-4 | 3.9e-3 | 0 / 267 (?) | 1 | 37 |
| slot − circle over cap | B: cavalier_contours | yes | 1.0e-7 | 6.4e-8 | 8.1e-7 | 4 / 2 (?) | 1 | 1 |
| slot − circle over cap | C: Locus-owned | yes | 1.0e-7 | 6.4e-8 | 8.1e-7 | 3 / 2 (?) | 1 | 9 |
| circle ∩ circle (lens) | A: i_overlay + refit | yes | 1.8e-4 | 1.9e-4 | 1.8e-3 | 2 / 0 (2 / 0) | 1 | 943 |
| circle ∩ circle (lens) | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 1.2e-2 | 0 / 187 (2 / 0) | 1 | 49 |
| circle ∩ circle (lens) | B: cavalier_contours | yes | 1.0e-7 | 2.6e-8 | 2.5e-6 | 4 / 0 (2 / 0) | 1 | 1 |
| circle ∩ circle (lens) | C: Locus-owned | yes | 1.0e-7 | 2.6e-8 | 2.5e-6 | 2 / 0 (2 / 0) | 1 | 6 |
| rect ∪ rect, shared edge | A: i_overlay + refit | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 4 (0 / 4) | 1 | 1 |
| rect ∪ rect, shared edge | A0: i_overlay lines | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 4 (0 / 4) | 1 | 1 |
| rect ∪ rect, shared edge | B: cavalier_contours | NO (sliver) | 5.0e0 | <1e-9 | 0.0e0 | 0 / 8 (0 / 4) | 2 | 1 |
| rect ∪ rect, shared edge | C: Locus-owned | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 4 (0 / 4) | 1 | 4 |
| rect ∪ rect, partial shared edge | A: i_overlay + refit | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 2 |
| rect ∪ rect, partial shared edge | A0: i_overlay lines | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 1 |
| rect ∪ rect, partial shared edge | B: cavalier_contours | NO (sliver) | 2.5e0 | <1e-9 | 0.0e0 | 0 / 10 (0 / 8) | 2 | 1 |
| rect ∪ rect, partial shared edge | C: Locus-owned | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 5 |
| rect ∪ rect, touching corner | A: i_overlay + refit | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 2 | 2 |
| rect ∪ rect, touching corner | A0: i_overlay lines | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 2 | 1 |
| rect ∪ rect, touching corner | B: cavalier_contours | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 1 |
| rect ∪ rect, touching corner | C: Locus-owned | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 2 | 4 |
| rect − rect, notch on edge | A: i_overlay + refit | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 2 |
| rect − rect, notch on edge | A0: i_overlay lines | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 1 |
| rect − rect, notch on edge | B: cavalier_contours | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 1 |
| rect − rect, notch on edge | C: Locus-owned | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 8 (0 / 8) | 1 | 5 |
| circle − concentric circle | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 4 / 0 (2 / 0) | 2 | 6568 |
| circle − concentric circle | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 1.0e-2 | 0 / 538 (2 / 0) | 2 | 41 |
| circle − concentric circle | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 4 / 0 (2 / 0) | 2 | 1 |
| circle − concentric circle | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 2 / 0 (2 / 0) | 2 | 4 |
| circle ∪ circle, external tangent | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 4.2e-6 | 4 / 0 (2 / 0) | 2 | 4426 |
| circle ∪ circle, external tangent | A0: i_overlay lines | yes | 4.9e-4 | 5.0e-4 | 2.1e-2 | 0 / 446 (2 / 0) | 2 | 32 |
| circle ∪ circle, external tangent | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 4.2e-6 | 4 / 0 (2 / 0) | 1 | 1 |
| circle ∪ circle, external tangent | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 4.2e-6 | 2 / 0 (2 / 0) | 2 | 4 |
| circle − circle, internal tangent | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 4 / 0 (2 / 0) | 1 | 6641 |
| circle − circle, internal tangent | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 1.0e-2 | 0 / 538 (2 / 0) | 1 | 41 |
| circle − circle, internal tangent | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 4 / 0 (2 / 0) | 1 | 1 |
| circle − circle, internal tangent | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 2.1e-6 | 2 / 0 (2 / 0) | 2 | 4 |
| slot ∪ circle on its cap (coincident arc) | A: i_overlay + refit | yes | 1.0e-7 | 1.0e-7 | 2.1e-6 | 3 / 2 (2 / 2) | 1 | 1269 |
| slot ∪ circle on its cap (coincident arc) | A0: i_overlay lines | yes | 4.9e-4 | 4.9e-4 | 9.5e-3 | 0 / 556 (2 / 2) | 1 | 178 |
| slot ∪ circle on its cap (coincident arc) | B: cavalier_contours | yes | 1.0e-7 | 1.0e-7 | 2.1e-6 | 3 / 2 (2 / 2) | 1 | 1 |
| slot ∪ circle on its cap (coincident arc) | C: Locus-owned | yes | 1.0e-7 | 1.0e-7 | 2.1e-6 | 2 / 2 (2 / 2) | 1 | 7 |
| circle ∪ same circle | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 2.9e-6 | 2 / 0 (1 / 0) | 1 | 3177 |
| circle ∪ same circle | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 1.5e-2 | 0 / 263 (1 / 0) | 1 | 24 |
| circle ∪ same circle | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 2.9e-6 | 2 / 0 (1 / 0) | 1 | 0 |
| circle ∪ same circle | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 2.9e-6 | 1 / 0 (1 / 0) | 1 | 3 |
| rect ∩ circle, tangent at a point | A: i_overlay + refit | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 0 (0 / 0) | 0 | 21 |
| rect ∩ circle, tangent at a point | A0: i_overlay lines | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 0 (0 / 0) | 0 | 14 |
| rect ∩ circle, tangent at a point | B: cavalier_contours | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 0 (0 / 0) | 0 | 0 |
| rect ∩ circle, tangent at a point | C: Locus-owned | yes | <1e-9 | <1e-9 | 0.0e0 | 0 / 0 (0 / 0) | 0 | 5 |
| rounded rect − slot inside (hole) | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 4.2e-7 | 8 / 6 (6 / 6) | 2 | 1118 |
| rounded rect − slot inside (hole) | A0: i_overlay lines | yes | 4.9e-4 | 4.9e-4 | 2.1e-3 | 0 / 380 (6 / 6) | 2 | 28 |
| rounded rect − slot inside (hole) | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 4.2e-7 | 6 / 6 (6 / 6) | 2 | 1 |
| rounded rect − slot inside (hole) | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 4.2e-7 | 6 / 6 (6 / 6) | 2 | 12 |
| annulus ∪ bar (holed operand) | A: i_overlay + refit | yes | 1.7e-4 | 1.7e-4 | 2.5e-3 | 4 / 8 (4 / 8) | 3 | 2621 |
| annulus ∪ bar (holed operand) | A0: i_overlay lines | yes | 5.0e-4 | 5.0e-4 | 1.0e-2 | 0 / 452 (4 / 8) | 3 | 53 |
| annulus ∪ bar (holed operand) | B: cavalier_contours | n/a | | | | | | |
| annulus ∪ bar (holed operand) | C: Locus-owned | yes | 1.0e-7 | 8.8e-8 | 2.1e-6 | 4 / 8 (4 / 8) | 3 | 19 |

### Stress

| Fixture | Option | Pass | Dev out→ref (mm) | Dev ref→out (mm) | ΔArea (mm²) | Arcs / lines (expected) | Loops | µs |
|---|---|---|---|---|---|---|---|---|
| plate − 50 holes | A: i_overlay + refit | yes | 1.0e-7 | <1e-9 | 4.0e-5 | 105 / 4 (54 / 4) | 51 | 46788 |
| plate − 50 holes | A0: i_overlay lines | yes | 4.9e-4 | 5.0e-4 | 2.0e-1 | 0 / 7278 (54 / 4) | 51 | 684 |
| plate − 50 holes | B: cavalier_contours | yes | 1.0e-7 | <1e-9 | 4.0e-5 | 104 / 4 (54 / 4) | 51 | 46 |
| plate − 50 holes | C: Locus-owned | yes | 1.0e-7 | <1e-9 | 4.0e-5 | 54 / 4 (54 / 4) | 51 | 701 |
| chain of 40 circles ∪ | A: i_overlay + refit | yes | 2.0e-4 | 2.0e-4 | 1.4e-2 | 78 / 0 (78 / 0) | 1 | 9610 |
| chain of 40 circles ∪ | A0: i_overlay lines | yes | 4.9e-4 | 4.9e-4 | 1.2e-1 | 0 / 3371 (78 / 0) | 1 | 787 |
| chain of 40 circles ∪ | B: cavalier_contours | yes | 1.0e-7 | 8.2e-8 | 2.4e-5 | 80 / 0 (78 / 0) | 1 | 241 |
| chain of 40 circles ∪ | C: Locus-owned | yes | 1.0e-7 | 8.2e-8 | 2.4e-5 | 78 / 0 (78 / 0) | 1 | 1298 |
| disc ∪ 60 teeth | A: i_overlay + refit | yes | 2.8e-4 | 2.0e-4 | 1.3e-2 | 120 / 0 (120 / 0) | 1 | 11590 |
| disc ∪ 60 teeth | A0: i_overlay lines | yes | 6.9e-4 | 5.0e-4 | 9.1e-2 | 0 / 3563 (120 / 0) | 1 | 1211 |
| disc ∪ 60 teeth | B: cavalier_contours | yes | 1.0e-7 | 1.4e-7 | 1.8e-5 | 182 / 0 (120 / 0) | 1 | 767 |
| disc ∪ 60 teeth | C: Locus-owned | yes | 1.0e-7 | 1.4e-7 | 1.8e-5 | 120 / 0 (120 / 0) | 1 | 2546 |

### Random sweep: 0.5 mm grid (exact coincidences and tangencies) (5000 cases)

| Option | Pass | Fail | Panic | n/a | Worst finite dev (mm) | Arcs / lines total |
|---|---|---|---|---|---|---|
| A: i_overlay + refit | 4989 | 11 | 0 | 0 | 2.8e0 | 11971 / 15867 |
| A0: i_overlay lines | 4971 | 29 | 0 | 0 | 2.8e0 | 0 / 489092 |
| B: cavalier_contours | 4946 | 54 | 0 | 0 | 1.8e0 | 12075 / 16108 |
| C: Locus-owned | 5000 | 0 | 0 | 0 | 1.6e-4 | 10354 / 15797 |

### Random sweep: 0.5 mm grid, operand B shifted by up to 1e-7 mm (5000 cases)

| Option | Pass | Fail | Panic | n/a | Worst finite dev (mm) | Arcs / lines total |
|---|---|---|---|---|---|---|
| A: i_overlay + refit | 4964 | 36 | 0 | 0 | 4.5e0 | 11986 / 15931 |
| A0: i_overlay lines | 4949 | 51 | 0 | 0 | 4.5e0 | 0 / 489641 |
| B: cavalier_contours | 4922 | 78 | 0 | 0 | 4.5e0 | 12185 / 16182 |
| C: Locus-owned | 5000 | 0 | 0 | 0 | 5.1e-4 | 10354 / 15851 |

### Random sweep: general position (random floats) (5000 cases)

| Option | Pass | Fail | Panic | n/a | Worst finite dev (mm) | Arcs / lines total |
|---|---|---|---|---|---|---|
| A: i_overlay + refit | 4991 | 9 | 0 | 0 | 2.7e-3 | 12088 / 16432 |
| A0: i_overlay lines | 4945 | 55 | 0 | 0 | 6.2e-3 | 0 / 494049 |
| B: cavalier_contours | 5000 | 0 | 0 | 0 | 1.0e-6 | 12609 / 16316 |
| C: Locus-owned | 5000 | 0 | 0 | 0 | 1.0e-6 | 10788 / 16316 |
