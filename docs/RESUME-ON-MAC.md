# Resuming ABRP-native on a Mac

Written 27 Aug 2026 when development moved from the Windows box to a Mac. Read this first, then
`CLAUDE.md`, then the wayfinder map.

## 1. What this project is, in one paragraph

A native iOS replication of ABRP's core EV planner for one car (Hyundai Ioniq 5, 2022), Benelux/FR/DE
first, personal learning project. Architecture is **decided** (six ADRs in `docs/adr/`); the route is
planned as a *wayfinder map* on GitHub issue
[#1](https://github.com/arodroz/abrp/issues/1). Nothing has been built yet — the next step is a
measured vertical-slice prototype, which is exactly what needed a Mac.

## 2. Get the machine ready (once)

```bash
# tooling
xcode-select --install            # or Xcode 16+ from the App Store, then accept its licence
brew install gh git python@3.13
gh auth login                     # account ilres-antonio; needed for the issue tracker
git clone https://github.com/arodroz/abrp.git && cd abrp

# Claude Code + the skills this project relies on
npm install -g @anthropic-ai/claude-code
```

Then run the environment bootstrap that the Mac ticket asks for:

```bash
bash scripts/bootstrap-mac.sh     # rustup + iOS targets, UniFFI, osmium/pmtiles/aws, Geofabrik data
```

and paste its report into `docs/research/dev-environment.md`.

### Skills to bring over

The wayfinder workflow lives in **user-level** skills, not in the repo. Copy these folders from the
Windows box `C:\Users\antonio\.claude\skills\` to `~/.claude/skills/` on the Mac (or re-run
`/setup-matt-pocock-skills`):

`wayfinder`, `grilling`, `domain-modeling`, `prototype`, `research`, `grill-with-docs`, `to-tickets`,
`triage`, `handoff`.

Also copy the project memory folder
`C:\Users\antonio\.claude\projects\C--Users-antonio-Downloads-abrp\memory\` — its target name on the Mac
is derived from the new checkout path (`~/.claude/projects/-Users-<you>-…-abrp/memory/`), so create it
after the first Claude session there and move the files in.

Plugins used: the `github` plugin (issue MCP tools — optional, `gh` CLI covers everything) and
`context7` (library docs). Nothing Windows-specific is required; the Odin MCP server is unrelated to this project.

## 3. Where things are

| What | Where |
|---|---|
| Glossary (domain terms, canonical names) | `CONTEXT.md` |
| Architecture decisions | `docs/adr/0001…0006` |
| Research notes (inputs to the ADRs) | `docs/research/*.md` |
| Third-party attribution | `NOTICE.md` |
| How the issue tracker is used (map, sub-issues, blocking) | `docs/agents/issue-tracker.md` |
| The map itself | GitHub issue #1, label `wayfinder:map` |
| Mac bootstrap | `scripts/bootstrap-mac.sh` |

## 4. State of the map on 27 Aug 2026

Decided and closed: where routing runs (on-device Rust CH over Region Packs), map engine (MapLibre
Native + Protomaps PMTiles Map Packs), Energy Model form, Rust/Swift boundary (UniFFI `Planner`),
data sources, core planner scope, Charging Stop optimiser, Map Pack sizes.

Open, in dependency order:

1. **Task: macOS build environment** (#24) — *you, on this Mac*: Xcode, paired ProMotion iPhone,
   `bash scripts/bootstrap-mac.sh`, MapLibre blank app at 120 fps, fill `dev-environment.md`, close.
2. **Task: verify open-data licences** (#21) — *you, in a browser*: transportdata.be licence text,
   mobilithek.info AFIR offers; checklist and an e-mail draft are on the ticket. Blocks nothing.
3. **Prototype: vertical slice benchmark** (#15) — LU → ~400 km Plan with Charging Stops on the
   iPhone; measures the <1 s Plan / 120 fps / <1 GB bars from ADR 0001/0002. Unblocked by (1).
4. After the prototype: Region Pack format (#16), turn-restriction research (#17), Map Pack packaging
   (#19), Energy Model calibration (#20), planner UI prototype (#23).

Both tasks are currently **assigned to ilres-antonio** as claims from the Windows session; keep or
reassign, they are yours either way.

## 5. How to continue

```
claude                       # in the repo
/wayfinder 1                 # picks the first open, unblocked, unclaimed ticket and works it
/wayfinder 1  (after closing #24)  → runs the prototype ticket with the `prototype` skill
```

Conventions the agent follows on this map (see `CLAUDE.md` and the map's Notes): one ticket per
session; decisions become ADRs + glossary entries; never reuse Iternio/ABRP data or private APIs;
Rust only where measured necessary; refer to tickets by title.

## 6. Differences from the Windows session

- Line endings: `.gitattributes` forces LF on `*.sh`/`*.md`; nothing to configure.
- The Windows box keeps only Python 3.13 and the `pmtiles` CLI; it is no longer needed for anything.
- Scratch files from the Windows sessions (country GeoJSON outlines, `pmtiles.exe`, the LU z15 extract)
  were in a temp folder and are not in the repo; regenerate with `pmtiles extract` if needed
  (`docs/research/map-pack-sizes.md` has the exact commands).
