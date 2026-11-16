# 3-D rain: native rain streaks and splashes, opt-in

Status: implemented, not yet built or measured.
Module: `engine/nvforever/src/rain3d.rs`. Flag: `--rain-3d [--rain-3d-density <n>]`.
Splashes on water: the existing ripple height field (`ripples.rs`); on land: ring quads
in the same batched mesh.

This is the feature of **3D Rain** (Nexus 79652) rebuilt without an NVSE plugin: many
falling streaks in a volume around the camera, moved by the weather's wind, with splashes
where they land, instead of the few large animated planes the original's rain uses. The
engine already draws a weather's precipitation model when a weather has one
(`weather_fx.rs`), and the only rainy weathers in the ten masters are Honest Hearts' two
Zion rain weathers — which in the original are drawn by a placed model the rain script
moves, not by `WTHR MODL` [documented: docs/spec-notes/world.md §6]. So this is an
**opt-in native effect**, not a data-driven one: with the flag off nothing changes.

## 1. What is drawn

`[design]` One fixed budget of streaks lives in a box that travels with the main camera
(`fly::WorldCamera`): half-width 2048 game units, 900 above and 350 below the camera. A
streak stores its place as an **offset from the camera** and is wrapped into the box each
frame (`wrap`, `wrap_z`), so the budget covers every view: flying across the map, looking
up, or standing still cost the same. This is what "camera-relative" means here. A unit is
about 1.4 cm (a 112-unit eye height is 1.6 m), so the box is roughly 58 m across.

- **Motion.** Each streak falls at 850–1450 units/s (its own speed), plus the weather's
  wind — `tree::wind_vector`, the same direction and `WTHR DATA` wind speed the trees and
  grass fires take — scaled so a 255/255 wind is 700 units/s, with a slow two-sine gust.
  A streak's speed over its own length is what reads as a 15–40 ms exposure.
- **Look.** One unlit `StandardMaterial`, alpha-blended, double-sided, no fog, with a
  procedurally made 128² atlas: a soft vertical streak (left half) and a soft ring (right
  half), white so the vertices tint them. The rain's display colour is pale grey by day and
  dark blue-grey at night (`tint`, `SkyState::daylight`), and the alpha is
  `STREAK_ALPHA × weather strength × per-streak share × near fade × far fade`. A streak
  nearer than 40 units fades out (a drop in front of the lens must not be a screen-filling
  quad); the far fade runs 1400 → 2048 units, so nothing pops at the box's edge.
- **One draw call.** Every streak and splash ring is a quad in **one** `Mesh`, rebuilt on
  the CPU each frame and swapped into the same `Mesh` asset (both vertex sets' allocations
  are kept, as `particles.rs` does). The entity carries `NoFrustumCulling` and
  `NotShadowCaster`; it is hidden while no rain falls.
- **Splashes.** 120 impacts/s at density 1 (`SPLASHES_PER_SECOND`) are sampled on a
  1200-unit disc around the camera. The point's surface is read the way the terrain and
  grass read it: the visible water surface first (`DistantWater::wet_area`/`surface`), else
  the LAND height (`land::load_land`, heights kept per cell in `Lands`). An impact on water
  is **not** a ring: it is queued into the water's own ripple simulation
  (`Ripples::splash`), so the splash is a wave in the height field the water shader already
  reads (the grid covers 1024 units around the camera, so a water impact outside that
  square is simply not seen — it is outside the water's own splash field). An impact on
  land is a small horizontal ring quad that grows and fades over 0.5 s, lifted 1.5 units
  above the sampled height. Rings are capped at 256 alive.

Why rings for land and ripples for water: the ripple grid is already the engine's water
splash language and costs nothing extra (it runs for rain today); a ring quad on water
would z-fight the water plane and duplicate that language. On land there is no height field
to write to, and a real decal per impact (Bevy `ForwardDecal`, as `decals.rs` uses) is one
entity, material and depth-prepass decal per drop, capped at 64 — far too few and too
heavy for rain. A batched quad is the cheap answer.

## 2. GPU or CPU: the decision

**Chosen: CPU simulation, one GPU draw.** `[design]`

The draw is already GPU-friendly in the ways that matter: one mesh, one material, one draw
call, no per-quad entity, no bind groups per drop, no per-drop uniform. The per-frame work
that a fully GPU path would remove is the CPU loop and the vertex buffer upload:

- 4000 streaks (default) build 16 000 vertices / 24 000 indices a frame; the cap (8000)
  builds 32 000 / 48 000. Positions + normals + UVs + colours are 48 bytes a vertex,
  plus 4 bytes an index: about 0.9 MB a frame at the default, 1.8 MB at the cap [design
  arithmetic], uploaded through the same `Mesh` asset the particle systems use.
- A GPU path would need a custom pipeline or material extension to generate quads from an
  instance buffer and a time uniform, integrated with Bevy 0.20.1's prepass, MSAA and
  tonemapping paths — a render-graph change for a feature whose visual cost is **fill
  rate, not vertex count**. A streak costs the same pixels however it was positioned.
- The CPU path is deterministic, testable in a unit test, and keeps the weather, wind,
  splash-surface and density decisions in Rust where the rest of the weather system is.

If profiling (see §5) ever shows the CPU loop as the cost, the upgrade is local: keep the
same per-streak state, add an instance buffer and a vertex shader that expands a 6-vertex
quad. Nothing in this design depends on the quad being built on the CPU.

## 3. Default off, and bit-identical

`[design]` `--rain-3d` is a `bool` defaulting to false. `Rain3dPlugin(None)` returns from
`build` without adding anything: no systems, no resource, no image, mesh or material, no
random draw and no log line. `main.rs` still parses the two fields, which changes no
existing run's behaviour. `ripples.rs` gains two fields defaulting to false/empty and one
`if !rp.rain3d` guard around its existing synthetic rain drops: with the flag off the
guard is always taken and the queued-impacts loop iterates an empty vector, so the
simulation, the random stream and the uploaded heights are exactly as before. `weather_fx.rs`
checks for the `NativeRain` marker resource, which only `Rain3dPlugin` inserts; without the
flag the check is `None` and every branch is as before. There is no golden or scenario
change.

With the flag on, the weather's precipitation **model** is not drawn while it precipitates
(`weather_fx` sets `want_precip = None` when `NativeRain` exists): the streaks replace the
model's big planes, which is the point of the mod. Its precipitation **sound** still loops
— only the visual planes are replaced. Honest Hearts' rain is drawn by the rain script's
placed `NVDLC02Rain` activators, not by `WTHR MODL`; those references still draw (they are
script-driven game objects) [hypothesis: the original 3D Rain mod leaves them too; hiding
them would mean matching placed references by model path, which this change does not do].

## 4. When it rains

`[design]` `precipitation_strength(sky, forced)` is the same rule `weather_fx` follows for
the precipitation model, from the shown and outgoing weathers:

- outdoors only (`SkyState::interior` is `None`);
- a weather precipitates when its `DATA` has the `Rainy` bit **or** it names a `MODL`
  precipitation model (`Weather::rainy`, `Weather::precipitation_model`). Vanilla data has
  no `MODL`, so the `Rainy` bit is what makes the two Zion weathers rain; a mod's snow
  weather is drawn as rain (the data has no rain/snow distinction beyond the classification
  byte) [hypothesis];
- during a change, the incoming weather's precipitation **begin-fade-in** byte and the
  outgoing weather's **end-fade-out** byte decide, through `weather_fx::fade_share_on` —
  so rain starts and stops exactly where the engine's precipitation model would.

The binary target is eased over 0.4 s (`RAMP_SECONDS`) so the fade byte does not pop; the
strength multiplies every streak's and splash's alpha and the splash rate. The wind comes
from `SkyState` every frame, so a weather change bends the rain with the trees.

`NVF_PLAY_TEST_WEATHER_FX=precip=<model>` (the existing test aid) is read here too: any
weather then precipitates, for a screenshot without waiting for Zion rain.

## 5. Cost, and how to measure it

The hard cap is **8000 streaks + 256 splash rings = 8256 quads**, 16 512 triangles, one
draw call; the default 4000 streaks is **4256 quads**, 8512 triangles. State (game units,
one draw call, alpha blended, depth tested):

| At density 1 (4000 streaks) | Estimate |
|---|---|
| CPU: simulation + quad build + mesh swap | 0.3–0.8 ms a frame [hypothesis] |
| GPU: vertices | 16 000, 24 000 indices a frame (one call) |
| GPU: blended pixels | 0.2–0.6 Mpx at 1280×800, of 1.02 Mpx [hypothesis: mean 50–150 px a streak, most of the box outside the frustum] |
| Upload | ~0.9 MB a frame (~110 MB/s at 120 fps) |
| At the cap (8000) | about twice each line, still one call |

The 120 fps target is guarded by the cap rather than by per-view culling: a streak outside
the frustum costs a few vertex shader invocations, never fill. The fill estimate is the
number to check: rain is alpha-blended overdraw, and MSAA multiplies it (`--msaa 4`).

A/B with the engine's own frame-time aid (no external `perf.sh` exists in this tree; pass
these through whatever runner the machine uses, `--log-frame-times 5` logs mean and p99
every 5 s and turns vsync off):

```sh
D="$HOME/Games/Heroic/FalloutNewVegas/Fallout New Vegas English/Data"   # your install
# Forced rain (the two Zion weathers are Rainy; this also works on a clear weather
# because the test aid makes every weather precipitate):
export NVF_PLAY_TEST_WEATHER_FX='precip=nvdlc02\sky\nvdlc02rain.nif'

# A: off (today's picture), at 1280x800 and at the display's native mode.
cargo run --release -p nvforever -- --data "$D" --cell Goodsprings --play \
  --hour 12 --weather NVDLC02ZVRainWeather --no-voice --no-messages \
  --log-frame-times 5 --size 1280x800 --run-seconds 60
cargo run --release -p nvforever -- --data "$D" --cell Goodsprings --play \
  --hour 12 --weather NVDLC02ZVRainWeather --no-voice --no-messages \
  --log-frame-times 5 --window borderless --run-seconds 60

# B: on at density 1, then at the hard cap (density 2).
cargo run --release -p nvforever -- --data "$D" --cell Goodsprings --play \
  --hour 12 --weather NVDLC02ZVRainWeather --no-voice --no-messages \
  --rain-3d --log-frame-times 5 --size 1280x800 --run-seconds 60
cargo run --release -p nvforever -- --data "$D" --cell Goodsprings --play \
  --hour 12 --weather NVDLC02ZVRainWeather --no-voice --no-messages \
  --rain-3d --rain-3d-density 2 --log-frame-times 5 --size 1280x800 --run-seconds 60
# and the same two with --window borderless for the native frame.
```

Read the `frame times:` lines (`--log-frame-times`); compare mean and p99, on an otherwise
idle machine, and repeat each leg twice. A synthetic GPU number can also come from
`NVF_PLAY_TEST_PROFILE=5` with the `profile` feature: look for the frame's render time and
the `rain3d` spans in the CPU list (`update` is one system).

Screenshot (same aid, works with or without `--play`; a `--screenshot` run waits for the
cells around the start):

```sh
NVF_PLAY_TEST_WEATHER_FX='precip=nvdlc02\sky\nvdlc02rain.nif' \
cargo run --release -p nvforever -- --data "$D" --cell Goodsprings --play \
  --hour 12 --weather NVDLC02ZVRainWeather --rain-3d --no-voice --no-messages \
  --size 1280x800 --screenshot rain-3d.png --screenshot-delay 6
```

What the screenshot should show: many thin streaks over the whole view, tilted downwind,
with small rings on the ground and no large rain planes. What the log should show:
`rain3d: rain on in NVDLC02ZVRainWeather (4000 streaks, …)`; with the flag off, no
`rain3d:` line anywhere.

## 6. Known limits

- **No roof test.** The volume is around the camera, so rain falls through a porch roof
  while the player stands under it outdoors. The original's placed planes have the same
  problem; a per-impact line cast against the drawn world (as `decals.rs` does for actors)
  would be the next step.
- **Ground sampling is per impact, not per streak.** Most visible streaks do not have a
  matching splash ring; the two systems are independent budgets. Sampling an impact per
  streak would multiply ground queries and ring geometry for a detail that is not visible
  beyond a few metres.
- **Snow draws as rain** (see §4).
- **The game's own rain planes** (Honest Hearts' scripted activators) still draw with the
  flag on (see §3); a weather mod's `WTHR MODL` does not.
- **Interiors** show nothing — the strength is 0 indoors, the same rule the game's weather
  precipitation follows.
