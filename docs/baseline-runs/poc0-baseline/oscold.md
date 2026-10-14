| run | first load s | reloads s | WrathStats load_save spans s | save total s (routine / commit) | save max frame ms |
|---|---|---|---|---|---|
| game-oscold | 12.24 | 9.34 / 8.97 / 9.26 | 12.09 / 9.34 / 8.98 / 9.26 | 0.473 (0.473 / 0.439) | 246.7 |
| best-oscold | 7.77 | 5.16 / 4.80 / 5.03 | 7.62 / 5.16 / 4.80 / 5.04 | 0.473 (0.473 / 0.438) | 250.9 |

| run | pass | frames | fps | p50 | p95 | p99 | max ms | ≥33 ms | ≥50 ms | GC frames |
|---|---|---|---|---|---|---|---|---|---|---|
| game-oscold | 1 | 24132 | 80.4 | 12.45 | 17.02 | 21.12 | 355.2 | 6 | 6 | 3 |
| game-oscold | 2 | 24142 | 80.5 | 12.47 | 16.97 | 21.06 | 169.0 | 2 | 2 | 2 |
| best-oscold | 1 | 24115 | 80.4 | 12.45 | 17.01 | 21.03 | 370.8 | 7 | 7 | 3 |
| best-oscold | 2 | 24143 | 80.5 | 12.45 | 17.04 | 21.08 | 93.9 | 1 | 1 | 2 |

| run | after-load resident GB / Mono heap GB | after-route1 resident GB / Mono heap GB | after-reload resident GB / Mono heap GB | after-route2 resident GB / Mono heap GB | before-save resident GB / Mono heap GB | after-save resident GB / Mono heap GB |
|---|---|---|---|---|---|---|
| game-oscold | 3.49 / 0.47 | 3.52 / 0.47 | 3.59 / 0.48 | 3.63 / 0.52 | 3.66 / 0.50 | 3.67 / 0.50 |
| best-oscold | 3.50 / 0.47 | 3.52 / 0.47 | 3.60 / 0.49 | 3.63 / 0.52 | 3.65 / 0.51 | 3.66 / 0.50 |

Spread (median, min–max):
- first load: 10.01 (7.768–12.24, n=2) s
- warm reloads: 7.066 (4.802–9.338, n=6) s
- route pass 1 fps: 80.4 (80.37–80.42, n=2)
- route pass 1 p50_ms: 12.45 (12.45–12.45, n=2)
- route pass 1 p95_ms: 17.01 (17.01–17.02, n=2)
- route pass 1 p99_ms: 21.07 (21.03–21.12, n=2)
- route pass 1 max_ms: 363 (355.2–370.8, n=2)
- route pass 1 over_33ms: 6.5 (6–7, n=2)
- route pass 1 over_50ms: 6.5 (6–7, n=2)
- route pass 2 fps: 80.46 (80.46–80.47, n=2)
- route pass 2 p50_ms: 12.46 (12.45–12.47, n=2)
- route pass 2 p95_ms: 17 (16.97–17.04, n=2)
- route pass 2 p99_ms: 21.07 (21.06–21.08, n=2)
- route pass 2 max_ms: 131.5 (93.9–169, n=2)
- route pass 2 over_33ms: 1.5 (1–2, n=2)
- route pass 2 over_50ms: 1.5 (1–2, n=2)
- save total: 0.4728 (0.4727–0.4729, n=2) s
- resident at after-load: 3.498 (3.494–3.501, n=2) GB; Mono heap 0.4678 (0.4657–0.4699, n=2) GB
- resident at after-route1: 3.524 (3.523–3.525, n=2) GB; Mono heap 0.4678 (0.4657–0.4699, n=2) GB
- resident at after-reload: 3.595 (3.594–3.596, n=2) GB; Mono heap 0.4846 (0.4825–0.4867, n=2) GB
- resident at after-route2: 3.633 (3.632–3.634, n=2) GB; Mono heap 0.5182 (0.5161–0.5203, n=2) GB
- resident at before-save: 3.656 (3.654–3.658, n=2) GB; Mono heap 0.5029 (0.4981–0.5077, n=2) GB
- resident at after-save: 3.664 (3.659–3.669, n=2) GB; Mono heap 0.4977 (0.4955–0.5, n=2) GB
