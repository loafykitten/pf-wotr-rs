| run | first load s | reloads s | WrathStats load_save spans s | save total s (routine / commit) | save max frame ms |
|---|---|---|---|---|---|
| game-1 | 10.80 | 9.07 / 8.97 / 9.32 | 10.63 / 9.07 / 8.97 / 9.32 | 0.473 (0.473 / 0.440) | 252.9 |
| game-2 | 10.27 | 8.93 / 8.86 / 9.16 | 10.13 / 8.93 / 8.87 / 9.16 | 0.456 (0.456 / 0.423) | 244.3 |
| game-3 | 9.94 | 9.14 / 8.89 / 9.20 | 9.80 / 9.15 / 8.90 / 9.20 | 0.465 (0.465 / 0.430) | 239.2 |

| run | pass | frames | fps | p50 | p95 | p99 | max ms | ≥33 ms | ≥50 ms | GC frames |
|---|---|---|---|---|---|---|---|---|---|---|
| game-1 | 1 | 24199 | 80.7 | 12.45 | 17.01 | 21.08 | 41.8 | 1 | 0 | 4 |
| game-1 | 2 | 24164 | 80.5 | 12.46 | 16.97 | 21.01 | 22.1 | 0 | 0 | 2 |
| game-2 | 1 | 24205 | 80.7 | 12.46 | 17.03 | 21.10 | 26.3 | 0 | 0 | 4 |
| game-2 | 2 | 24136 | 80.4 | 12.47 | 16.98 | 21.07 | 22.8 | 0 | 0 | 2 |
| game-3 | 1 | 24208 | 80.7 | 12.45 | 17.02 | 21.09 | 27.2 | 0 | 0 | 4 |
| game-3 | 2 | 24180 | 80.6 | 12.46 | 16.98 | 21.05 | 22.1 | 0 | 0 | 2 |

| run | after-load resident GB / Mono heap GB | after-route1 resident GB / Mono heap GB | after-reload resident GB / Mono heap GB | after-route2 resident GB / Mono heap GB | before-save resident GB / Mono heap GB | after-save resident GB / Mono heap GB |
|---|---|---|---|---|---|---|
| game-1 | 3.50 / 0.46 | 3.52 / 0.46 | 3.59 / 0.49 | 3.63 / 0.53 | 3.64 / 0.51 | 3.65 / 0.49 |
| game-2 | 3.50 / 0.47 | 3.52 / 0.47 | 3.60 / 0.48 | 3.64 / 0.52 | 3.66 / 0.51 | 3.66 / 0.50 |
| game-3 | 3.50 / 0.47 | 3.53 / 0.47 | 3.60 / 0.48 | 3.64 / 0.52 | 3.65 / 0.51 | 3.65 / 0.50 |

Spread (median, min–max):
- first load: 10.27 (9.938–10.8, n=3) s
- warm reloads: 9.07 (8.863–9.32, n=9) s
- route pass 1 fps: 80.67 (80.65–80.68, n=3)
- route pass 1 p50_ms: 12.45 (12.45–12.46, n=3)
- route pass 1 p95_ms: 17.02 (17.01–17.03, n=3)
- route pass 1 p99_ms: 21.09 (21.08–21.1, n=3)
- route pass 1 max_ms: 27.23 (26.32–41.79, n=3)
- route pass 1 over_33ms: 0 (0–1, n=3)
- route pass 1 over_50ms: 0 (0–0, n=3)
- route pass 2 fps: 80.54 (80.44–80.59, n=3)
- route pass 2 p50_ms: 12.46 (12.46–12.47, n=3)
- route pass 2 p95_ms: 16.98 (16.97–16.98, n=3)
- route pass 2 p99_ms: 21.05 (21.01–21.07, n=3)
- route pass 2 max_ms: 22.09 (22.05–22.81, n=3)
- route pass 2 over_33ms: 0 (0–0, n=3)
- route pass 2 over_50ms: 0 (0–0, n=3)
- save total: 0.4648 (0.4559–0.4732, n=3) s
- resident at after-load: 3.501 (3.498–3.504, n=3) GB; Mono heap 0.4657 (0.4581–0.4657, n=3) GB
- resident at after-route1: 3.524 (3.523–3.529, n=3) GB; Mono heap 0.4657 (0.4581–0.4657, n=3) GB
- resident at after-reload: 3.601 (3.593–3.603, n=3) GB; Mono heap 0.4825 (0.4825–0.4917, n=3) GB
- resident at after-route2: 3.638 (3.63–3.64, n=3) GB; Mono heap 0.5161 (0.5161–0.5253, n=3) GB
- resident at before-save: 3.645 (3.641–3.663, n=3) GB; Mono heap 0.5068 (0.5056–0.5122, n=3) GB
- resident at after-save: 3.651 (3.647–3.66, n=3) GB; Mono heap 0.4973 (0.4908–0.4985, n=3) GB
