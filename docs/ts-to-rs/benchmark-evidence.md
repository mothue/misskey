# First port benchmark evidence

PASS: existing read-only infrastructure smoke and three alternating paired runs completed in an exclusive measurement window. Baseline is preserved `d875b7e3c737b7f9f75331fffb691f6a2b97f055`; candidate includes the final shutdown fix and Rust release executable. All474 HTTP differential cases also passed before measurement.

The temporary loopback RPC adds overhead for these tiny handlers: median p50 rose about0.16ms (+57–61% by median paired delta), and throughput fell31–33%. Performance improvement was not an acceptance gate. These short1000-request runs measure this local fixture/load configuration; their spread is reported, not extrapolated to production.

Every API/build/run uses100warmup+1000measured requests,concurrency8,existing phase/summary runner,one backend process,and identical dedicatedDB54412/Redis56412 fixture. A=baseline,B=candidate,actual execution orderAB/BA/AB. Startup requires both IPCready and HTTPhealth success. No builds/tests/federation jobs ran concurrently. Smoke ran timeline/global,note/show,user/show on both builds with the same100/1000/C8 settings:6000measured successes; paired APIs:18000measured successes. All warmup and measured error counts were0.

## Latency and throughput

Values are median across3runs with[min,max]. Latency is ms; throughput is successful requests/sec. Percent column is the median of per-pair `(candidate/baseline-1)*100`; absolute delta is candidate median minus baseline median. Population variance and complete per-pair values are preserved in [benchmark-results.json](benchmark-results.json).

|API|Metric|Baseline median[range]|Candidate median[range]|Absolute delta|Median paired delta%|
|---|---|---|---|---|---|
|ports/ping|p50|0.284[0.269,0.285]|0.447[0.446,0.482]|+0.163|+57.29%|
|ports/ping|p95|0.565[0.560,0.577]|0.756[0.726,1.043]|+0.191|+33.82%|
|ports/ping|p99|0.814[0.736,1.255]|1.234[0.910,1.524]|+0.420|+23.63%|
|ports/ping|throughput|24182.139[21566.617,24768.671]|16631.267[14546.081,16741.585]|-7550.872|-31.22%|
|ports/test|p50|0.264[0.255,0.277]|0.428[0.419,0.435]|+0.164|+58.64%|
|ports/test|p95|0.515[0.481,0.548]|0.660[0.657,0.690]|+0.145|+27.58%|
|ports/test|p99|0.683[0.552,0.758]|0.898[0.867,0.929]|+0.215|+26.85%|
|ports/test|throughput|26869.862[25802.679,27853.667]|17452.718[17135.110,17953.362]|-9417.144|-33.18%|
|ports/endpoint|p50|0.256[0.256,0.258]|0.412[0.407,0.415]|+0.155|+60.53%|
|ports/endpoint|p95|0.496[0.496,0.503]|0.645[0.633,0.666]|+0.149|+30.14%|
|ports/endpoint|p99|0.556[0.549,0.568]|0.812[0.736,0.854]|+0.256|+47.93%|
|ports/endpoint|throughput|26675.973[26436.318,26780.125]|18345.328[18162.156,18789.973]|-8330.646|-31.30%|

## Individual pairs

Each cell lists baseline→candidate, then absolute delta and percentage. This exposes run-to-run changes rather than hiding them behind aggregate medians.

|API|Pair/order|p50 ms|p95 ms|p99 ms|Requests/sec|
|---|---|---|---|---|---|
|ports/ping|1/AB|0.284→0.447;+0.163(+57.29%)|0.577→0.726;+0.149(+25.89%)|1.255→1.234;-0.022(-1.75%)|21566.617→16741.585;-4825.032(-22.37%)|
|ports/ping|2/BA|0.269→0.482;+0.213(+79.14%)|0.560→1.043;+0.483(+86.26%)|0.814→1.524;+0.711(+87.35%)|24768.671→14546.081;-10222.591(-41.27%)|
|ports/ping|3/AB|0.285→0.446;+0.162(+56.84%)|0.565→0.756;+0.191(+33.82%)|0.736→0.910;+0.174(+23.63%)|24182.139→16631.267;-7550.872(-31.22%)|
|ports/test|1/AB|0.264→0.419;+0.155(+58.64%)|0.515→0.657;+0.142(+27.58%)|0.683→0.867;+0.183(+26.85%)|26869.862→17953.362;-8916.501(-33.18%)|
|ports/test|2/BA|0.277→0.428;+0.151(+54.34%)|0.548→0.660;+0.112(+20.42%)|0.758→0.929;+0.172(+22.64%)|25802.679→17452.718;-8349.961(-32.36%)|
|ports/test|3/AB|0.255→0.435;+0.180(+70.78%)|0.481→0.690;+0.209(+43.52%)|0.552→0.898;+0.346(+62.71%)|27853.667→17135.110;-10718.557(-38.48%)|
|ports/endpoint|1/AB|0.256→0.407;+0.150(+58.73%)|0.496→0.633;+0.137(+27.54%)|0.556→0.736;+0.180(+32.37%)|26675.973→18789.973;-7886.001(-29.56%)|
|ports/endpoint|2/BA|0.256→0.412;+0.155(+60.53%)|0.496→0.645;+0.149(+30.14%)|0.549→0.812;+0.263(+47.93%)|26780.125→18345.328;-8434.797(-31.50%)|
|ports/endpoint|3/AB|0.258→0.415;+0.156(+60.59%)|0.503→0.666;+0.164(+32.51%)|0.568→0.854;+0.287(+50.49%)|26436.318→18162.156;-8274.162(-31.30%)|

## Memory and startup

RSS is a `ps` snapshot after each scenario, not peak/unique physical memory. TS and directly owned Rust PIDs are recorded in raw results and durable JSON. Values below are KiB median[min,max]. Baseline has no Rust child. Memory differences are observational snapshots, not an isolated allocation comparison.

|API|BaselineTS|CandidateTS|CandidateRust|
|---|---|---|---|
|ports/ping|634016[631888,638080]|617328[613776,643712]|4096[4064,4192]|
|ports/test|634352[633248,639088]|618224[614864,644864]|4144[4096,4240]|
|ports/endpoint|634464[633456,639744]|619424[615888,645296]|4208[4128,4272]|

Spawn-to-ready startup ms:baseline1059.005[1028.765,1130.612],candidate1052.883[1046.378,1055.540]. Startup is separate from request latency and does not include compilation.

## Build timing and reproducibility

Warm/incremental build walltimes, measured separately before the quiet window:baselineTSbundle2084ms;candidate locked-release native590ms;candidateTSbundle2062ms. All exited0. Dependencies,release artifacts and OS caches were retained, and E2E ran concurrently. These values are not cold/full build costs or a controlled build-speed comparison. The native wrapper uses `cargo build --locked --release`.

Reproduce from repository root after confirming61912free and other workloads stopped:

1. From `packages/backend`, rebuild with `bun run --bun rolldown -c --benchmark` after native build.
2. `bun scripts/benchmarks/ports/paired.ts candidate-http`
3. `bun scripts/benchmarks/ports/paired.ts smoke`
4. `bun scripts/benchmarks/ports/paired.ts measure`
5. `bun scripts/benchmarks/ports/summary.ts`

Ignored raw evidence: `built/ts-to-rs/paired-results.json`, `paired-summary.json`, `smoke-results.json`, `build-times.json`, corresponding build/run logs and per-server logs. Durable full per-pair summaries including errors,PID/RSS,startup and variance are in [benchmark-results.json](benchmark-results.json). The preserved baseline source/build/start command remains described in [baseline-evidence.md](baseline-evidence.md). All measurement processes were stopped after completion; no commit or push was made.
