<h1 align="center">ORBIT JUMPER</h1>

<p align="center"><em>Spacecraft concept simulation and defense. Keep your orbit if you can.</em></p>

<p align="center">
  <a href="https://edgarhsanchez.github.io/orbit_jumper/"><strong>▶&nbsp;&nbsp;PLAY IN YOUR BROWSER</strong></a>
  &nbsp;·&nbsp; desktop, iOS, Android — no install
</p>

<p align="center">
  <img src="docs/media/cockpit.png" alt="Cockpit view: heading tape, raider contacts with range/velocity/closing speed, chamfered console clusters, and a raider crossing the canopy" width="900">
</p>

Real orbital mechanics in a hostile system. Propulsion is deliberately weak
against interplanetary distances — getting anywhere means **hitching rides on
gravity**: click-and-hold a planet's orbit ring and the ship burns into a
capture, then the planet carries you around the system for free. Dive past a
body in free flight and its gravity — real, integrated — slings you out
faster. Meanwhile alien raiders hunt you in packs that scale with your level,
forever.

## The game

| | |
|---|---|
| <img src="docs/media/orbit-rings.png" alt="Orbit rings around a banded planetoid" width="440"> | **Ride the rings.** Every body advertises orbit rings — larger bodies throw larger rings, so a giant can be leapt onto from far away. Click and hold a ring; the ship plans the burn, captures, and rides. Energy prices every maneuver. |
| <img src="docs/media/cockpit.png" alt="Cockpit HUD with live targeting data" width="440"> | **Fly it from the cockpit.** A full 3D cockpit with heading tape, reticle, and a targeting stack that tracks every raider — range, velocity, closing speed — plus projected trajectories drawn in-world. Some controls only exist in here; others only exist in the tactical view. |
| <img src="docs/media/dogfight.png" alt="Jagged raider ships engaging the player at close range" width="440"> | **Fight off the raiders.** Alien packs spawn on a clock, match your velocity, lead their shots, and strafe when they get close. Lasers, missiles, gravity wells — bounties fund your upgrades. Packs grow with your pilot level, and leveling never ends. |
| <img src="docs/media/system-orbits.png" alt="System-scale view: full orbit tracks around the sun" width="440"> | **A whole universe on rails.** Deterministic f64 simulation at a fixed 60 Hz — celestials ride Kepler rails, the ship integrates. Zoom from cockpit to system scale; travel between systems and sectors indefinitely. |
| <img src="docs/media/ship-close.png" alt="Greebled ship close-up: truss spine, canopy, radiators" width="440"> | **Your ship, your build.** Three frames (DART / LANCE / HAMMER), paints, accents — greebled, technical hulls assembled from dozens of parts, seeded per style so every combination looks distinct. Craft and tier up shield plating, drives, collectors, command arrays. |

## Made for phones too

<p align="center">
  <img src="docs/media/phone-portrait.png" alt="Portrait phone layout with the vessel panel open" width="260">
  &nbsp;&nbsp;
  <img src="docs/media/phone-landscape.png" alt="Landscape phone layout with corner touch clusters" width="560">
</p>

The same build runs in a phone browser with touch controls — chamfered
console buttons with press feedback, laid out per orientation (portrait and
landscape are distinct layouts, relaid live on rotation). Thrust cluster
under the left thumb, weapons under the right.

## Controls

| Action | Desktop | Touch |
|---|---|---|
| Command an orbit | click + hold a body or ring | tap + hold |
| Thrust | arrows | **PRO / RET / IN / OUT** |
| Climb / dive (3D) | `E` / `Q` | **VERT+ / VERT−** |
| Cockpit ⇄ tactical | `F` | **VIEW** |
| Laser / missile | `Z` / `X` | **LAS / MSL** |
| Gravity wells | `C` / `V` | **PULL / PUSH** |
| Vessel / map / study | `Tab` / `M` / `S` | topbar buttons |
| Zoom | mouse wheel | — |

## Under the hood

- **[Bevy 0.19](https://bevy.org)** — ECS, 3D rendering, HDR + bloom
- **[bevy_pf](https://github.com/edgarhsanchez/bevy_pf)** — the entire HUD is XAML with data-bound view-models, including the animated console buttons (control templates, triggers, storyboards)
- **f64 simulation** — camera-relative f32 rendering keeps precision at Gm scales; sim state is `SimPos`/`SimVel` in meters
- **wasm** — the web build targets WebGL2 so it runs on iOS Safari and Android Chrome; a GitHub Actions workflow rebuilds and publishes on every push
- Procedural planetoids (displaced icospheres with vertex-color banding), greebled ships and raiders seeded per style — no art assets

The design reference lives in [docs/design.md](docs/design.md).

## Run it locally

```sh
cargo run --release -p oj_game
```

Or build the web bundle yourself (needs `wasm32-unknown-unknown`,
`wasm-bindgen-cli` 0.2.126, and optionally `wasm-opt`):

```sh
./web/build-wasm.sh   # outputs web/dist/
```
