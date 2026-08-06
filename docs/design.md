# orbit_jumper — design and architecture

2026-08-06. Foundation commit. Stack: Rust, Bevy 0.19, bevy_pf (XAML UI).
Two research passes back the two riskiest calls (rendering scale,
multiplayer); their sources are cited inline.

## The game in one paragraph

You fly a vessel between orbits — around planets, suns, through solar
systems, across galaxies — in an infinite deterministic universe. Energy is
life: thrust costs it, and you refill by orbiting suns. Hotter suns charge
faster and hit harder; shields gate which suns you survive, and a study
sensor tells you what you're about to orbit — if you take the time. Damage
is periodic and escalating near a sun: shields drain, then hull, then the
ship is gone forever, leaving wreckage other players can salvage into
materials for upgrades. Weapons (lasers, missiles, gravity force-fields
that pull or push), propulsion tiers (rocket, light, gravity drives),
achievements, and score chase out of that loop.

## Module map (workspace crates)

| crate | role | status |
|---|---|---|
| `oj_orbits` | Kepler rails, two-body gravity, symplectic integration, Hohmann dv. Pure f64, engine-free, 7 tests incl. ISS-period and LEO->GEO dv sanity checks. | implemented |
| `oj_universe` | Deterministic infinite universe: i64 sector grid -> galaxies -> systems -> sun (10 classes incl. neutron star, magnetar, black hole) + planets with real orbital elements and resource profiles. Seed-pure; 5 tests. | implemented |
| `oj_materials` | Elements -> alloys (blend + synergy bonus) -> recipe book gating 11 upgrade slots x 8 tiers on alloy properties. Aetherite gates graviton tech. 4 tests. | implemented |
| `oj_protocol` | Net seam: `PlayerId` (pubkey), snapshots, session events, signed global records. No transport — see Networking. | implemented |
| `oj_game` | The Bevy app: sim plugins + bevy_pf HUD. | walking skeleton |

In-game modules land as plugins in `oj_game`: `sim` (rails/gravity/energy/
hazard — implemented), then `achievements`, `upgrades`, `scorecard`,
`weapons`, `propulsion`, `study`, `salvage`, `net`.

## Simulation architecture

- **Two-tier physics.** Celestials ride rails: closed-form Kepler
  propagation, deterministic and O(1) — real ellipses obeying Kepler's
  laws. Ships integrate semi-implicit Euler (symplectic: energy bounded
  over long runs — tested) under the dominant body plus thrust and
  force-field accelerations. Patched-conic SOI (implemented in
  `oj_orbits::sphere_of_influence`) decides the dominant body.
- **f64 sim, camera-relative f32 render.** At 1e9 m, f32 resolves ~64 m —
  unusable. Simulation stays f64; each frame renders `(pos - anchor)`
  cast to f32. `big_space` has no Bevy 0.19 release yet (0.12 -> 0.18), so
  the pattern is hand-rolled (~50 lines) and swappable later.
- **Determinism is a design law.** Universe = pure function of seed;
  sim = fixed 60 Hz. Any peer derives the same map; only dynamic state
  ever crosses the wire; replays and wreck forensics come free.

## Rendering research (meshlet question)

Requested: meshlets for large object counts. Research verdict: **meshlets
are the wrong tool for object COUNT** — Bevy's virtual geometry
(`bevy::pbr::experimental::meshlet`) solves single-mesh triangle density,
requires `TEXTURE_INT64_ATOMIC` on Vulkan/Metal only (no DX12/WebGPU/wasm),
forces `Msaa::Off`, is opaque-only, has a churning asset format, and
carries higher base overhead than the standard path. What actually serves
thousands of asteroids/debris in 0.19: **automatic instancing** (shared
mesh+material), the GPU-driven pipeline (multidraw, GPU occlusion culling —
many_cubes 49 -> 19 ms in 0.19), `MeshTag` + storage buffer for
per-instance variation, and `VisibilityRange` HLOD with dithered
crossfade. Planets from orbit: cube-sphere + shallow LOD + Bevy's built-in
raymarched atmosphere (0.17+) — no maintained spherical-terrain crate
targets 0.19, so we borrow bevy_terrain's design, not its code. The
`meshlet` cargo feature exists in `oj_game` for a future desktop hero
asset; it is off by default and must never become a load-bearing
dependency. (Sources: MeshletPlugin docs; jms55 virtual-geometry posts
0.14-0.16; Bevy 0.16/0.19 release notes; big_space compat table.)

## Networking: thousands of players, P2P-preferred

Research verdict (full citations in the research log): no shipped MMO is
fully serverless, WebRTC full-mesh ceilings at ~8-16 peers, GGPO rollback
at ~4-8, and P2P zone-authority schemes are validated only below ~500
nodes with security as the known weak point. The honest architecture:

1. **Session tier — one solar system = one session, 8-32 players,**
   player-hosted listen server (bevy_replicon 0.41, Bevy 0.19, listen-server
   mode; or lightyear 0.28 host-server if we want built-in prediction and
   bandwidth priority). Star topology, not mesh.
2. **Transport — iroh 1.0** (QUIC): NAT hole-punching that works, relay
   fallback operated by n0 and self-hostable — the "relay federation" for
   free. Browser builds later ride matchbox/WebRTC (bevy_matchbox
   currently lags at Bevy 0.18) or WebTransport via lightyear.
3. **Scale = horizontal concurrency.** Thousands of players is thousands
   of concurrent system-sessions, discovered via iroh-gossip topics per
   region — never one shared simulation.
4. **Global tier** — achievements, wreckage, leaderboards: signed records
   (`oj_protocol::Signed`) gossiped via iroh-gossip/blobs for the
   eventually-consistent part, plus ONE thin referee service (RACS
   pattern) that never simulates, only validates signatures + session
   membership + plausibility bounds. A malicious host can spoil one
   session's fun; it cannot mint global currency.
5. **Host migration** is the ecosystem gap — nothing ships it. Plan:
   periodic authoritative snapshots gossiped to 1-2 successor peers;
   lightyear's authority-transfer if we adopt it.

`oj_protocol` is written to this shape today: identity is a public key,
global records carry deterministic context (seed, system, tick) so any
peer can re-validate plausibility.

## Game systems (spec level)

- **Energy**: thrust and weapons drain; orbit proximity to a sun charges
  at `class.harvest_rate()` (implemented). Black holes charge nothing —
  they pay in gravity assists.
- **Suns**: 10 classes, frequency-weighted (55% M-dwarfs, ~1% exotics).
  Class fixes required shield tier (0-8), hazard DPS (1-120/s, proximity
  scaled), harvest rate, and study time. Exotics read as ordinary B stars
  until studied — the study-skip gamble is real (implemented in
  `oj_universe`; hazard loop implemented in `sim`).
- **Damage**: periodic near-sun; shields absorb, then hull; 100% = ship
  forever lost + wreck spawn. Shields regenerate away from hazards.
- **Salvage**: wrecks and ring debris carry elements by the system's
  resource profile; collecting feeds the materials inventory.
- **Materials/upgrades**: alloys blend two elements with a synergy bonus
  for complementary pairs; recipes gate 11 slots x 8 tiers on property
  thresholds (thermal for shields, graviton for force fields...). Tier-3+
  force fields provably require Aetherite (tested).
- **Weapons**: laser (hitscan, energy), missile (integrated projectile,
  seeking), force-field projectile (radial gravity well, attract or
  repel — both signs; it perturbs ships, debris, and missiles alike since
  everything integrates the same accelerations).
- **Propulsion**: rocket (high thrust, high energy), light drive (low
  thrust, efficient), gravity drive (thrust scales near massive bodies;
  Aetherite-gated), plus procedurally-rolled exotic drives as rare drops.
- **Study**: hold-to-scan a sun; sensor tier shortens time; result
  reveals class + required shield tier in the HUD.
- **Scorecard**: per-run (distance, systems visited, suns survived,
  salvage value, kills) and lifetime totals; local ron persistence first,
  signed `ScoreFinal` records for the global board later.
- **Achievements**: definitions + progress tracked locally, unlocked ones
  become signed gossiped records so others' achievements are browsable.

## Feature ideas beyond the brief (for fun and longevity)

- Wormhole pairs between galaxies: fast travel, found only by studying
  black holes.
- Supernova season events: a studied B/O star goes critical on a
  deterministic timer, reshaping a sector and seeding rare elements.
- Ghost wrecks: a destroyed ship's final 30 s replay is stored in its
  wreck (deterministic sim makes this ~free) — salvagers watch the death.
- Lagrange trading posts: neutral zones at L4/L5 of giant planets.
- Pulsar navigation: exotic systems act as beacons revealing distant
  galaxy sectors when studied.
- Time-dilation scoring near black holes: survive closer, score
  multiplies — the leaderboard magnet.
- Contracts/bounties posted as signed records; factions as emergent
  reputation over them.

## Phases

- **P0 (this commit)**: workspace, pure cores + tests, walking-skeleton
  app: real system spawned from seed, rails, ship integration, energy/
  hazard loop, XAML HUD with per-property bindings.
- **P1**: controls/camera polish, study + salvage + materials inventory,
  upgrades applied to ship stats, per-run scorecard, wreck spawning.
- **P2**: weapons trio + damage between ships (local/practice targets),
  achievements, asteroid/debris fields via instancing + VisibilityRange.
- **P3**: multi-system travel (SOI transitions, system streaming,
  galaxy map UI in XAML), planets with atmosphere rendering.
- **P4**: networking per the plan above — replicon listen-server over
  iroh, session discovery, signed global records, referee service.
- **P5**: browser build (WebGPU path, matchbox transport when its 0.19
  release lands).

## Rules carried over from bevy_pf's project log

The real application is the benchmark; measure the curve, not one point;
verify in the running game, not a settled screenshot; a feature that is
not fun or fast in the game does not ship on the strength of a harness.
