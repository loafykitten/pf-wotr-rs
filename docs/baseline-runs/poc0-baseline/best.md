| run | first load s | reloads s | WrathStats load_save spans s | save total s (routine / commit) | save max frame ms |
|---|---|---|---|---|---|
| best-1 | 6.20 | 5.09 / 4.70 / 4.87 | 6.05 / 5.09 / 4.70 / 4.88 | 0.460 (0.460 / 0.427) | 240.4 |
| best-2 | 5.95 | 5.06 / 5.00 / 5.20 | 5.81 / 5.07 / 5.00 / 5.20 | 0.490 (0.490 / 0.450) | 248.4 |
| best-3 | 6.00 | 4.97 / 5.05 / 4.95 | 5.84 / 4.97 / 5.05 / 4.96 | 0.469 (0.469 / 0.433) | 247.3 |

| run | pass | frames | fps | p50 | p95 | p99 | max ms | ≥33 ms | ≥50 ms | GC frames |
|---|---|---|---|---|---|---|---|---|---|---|
| best-1 | 1 | 24225 | 80.7 | 12.45 | 17.01 | 21.05 | 26.4 | 0 | 0 | 3 |
| best-1 | 2 | 24170 | 80.6 | 12.45 | 17.03 | 21.06 | 28.0 | 0 | 0 | 2 |
| best-2 | 1 | 24199 | 80.6 | 12.45 | 17.03 | 21.06 | 23.5 | 0 | 0 | 3 |
| best-2 | 2 | 24149 | 80.5 | 12.46 | 17.03 | 21.05 | 26.8 | 0 | 0 | 2 |
| best-3 | 1 | 24231 | 80.8 | 12.45 | 17.03 | 21.10 | 25.9 | 0 | 0 | 3 |
| best-3 | 2 | 24190 | 80.6 | 12.45 | 17.01 | 21.04 | 27.4 | 0 | 0 | 2 |

| run | after-load resident GB / Mono heap GB | after-route1 resident GB / Mono heap GB | after-reload resident GB / Mono heap GB | after-route2 resident GB / Mono heap GB | before-save resident GB / Mono heap GB | after-save resident GB / Mono heap GB |
|---|---|---|---|---|---|---|
| best-1 | 3.51 / 0.46 | 3.53 / 0.46 | 3.58 / 0.49 | 3.62 / 0.53 | 3.61 / 0.51 | 3.62 / 0.50 |
| best-2 | 3.50 / 0.47 | 3.52 / 0.47 | 3.59 / 0.49 | 3.63 / 0.52 | 3.65 / 0.51 | 3.65 / 0.50 |
| best-3 | 3.50 / 0.46 | 3.52 / 0.46 | 3.58 / 0.49 | 3.62 / 0.53 | 3.63 / 0.50 | 3.64 / 0.50 |

Spread (median, min–max):
- first load: 5.996 (5.947–6.2, n=3) s
- warm reloads: 4.998 (4.701–5.195, n=9) s
- route pass 1 fps: 80.73 (80.65–80.76, n=3)
- route pass 1 p50_ms: 12.45 (12.45–12.45, n=3)
- route pass 1 p95_ms: 17.03 (17.01–17.03, n=3)
- route pass 1 p99_ms: 21.06 (21.05–21.1, n=3)
- route pass 1 max_ms: 25.92 (23.47–26.4, n=3)
- route pass 1 over_33ms: 0 (0–0, n=3)
- route pass 1 over_50ms: 0 (0–0, n=3)
- route pass 2 fps: 80.56 (80.48–80.62, n=3)
- route pass 2 p50_ms: 12.45 (12.45–12.46, n=3)
- route pass 2 p95_ms: 17.03 (17.01–17.03, n=3)
- route pass 2 p99_ms: 21.05 (21.04–21.06, n=3)
- route pass 2 max_ms: 27.37 (26.78–27.97, n=3)
- route pass 2 over_33ms: 0 (0–0, n=3)
- route pass 2 over_50ms: 0 (0–0, n=3)
- save total: 0.4687 (0.4602–0.49, n=3) s
- resident at after-load: 3.501 (3.498–3.507, n=3) GB; Mono heap 0.4599 (0.458–0.4715, n=3) GB
- resident at after-route1: 3.516 (3.516–3.525, n=3) GB; Mono heap 0.4599 (0.458–0.4715, n=3) GB
- resident at after-reload: 3.585 (3.581–3.594, n=3) GB; Mono heap 0.4915 (0.4882–0.4935, n=3) GB
- resident at after-route2: 3.623 (3.619–3.632, n=3) GB; Mono heap 0.5251 (0.5218–0.527, n=3) GB
- resident at before-save: 3.633 (3.614–3.65, n=3) GB; Mono heap 0.5067 (0.5041–0.5096, n=3) GB
- resident at after-save: 3.643 (3.616–3.655, n=3) GB; Mono heap 0.4992 (0.4974–0.5025, n=3) GB
