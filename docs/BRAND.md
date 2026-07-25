# AtlasDrive — Brand

The single source of truth for the product name, voice and palette. Anything
user-visible — window chrome, screens, CLI output, the DMG, documentation —
follows this file.

## Name

**AtlasDrive.** One word, capital A, capital D. Never "Atlas Drive", "Atlasdrive"
or "ATLASDRIVE" in prose.

The name is the product idea: an *atlas* of your photographs, indexed across
numbered *drives*. The catalogue is the map; the drives are the territory.

## Positioning line

> Every photograph you own, on one map.

## Tagline options

Primary, used under the name in the app and on the DMG:

> **A private atlas of every photograph you own.**

Alternatives, by context:

- Short, for window chrome and menus: *Your photographs, mapped.*
- Functional, for the App Store style short description:
  *Find any photograph across every drive — even the ones in the drawer.*
- Reassurance-first, where privacy is the concern:
  *Everything stays on your Mac. Originals are never touched.*

## Elevator paragraph

> AtlasDrive turns a shelf of external drives into one searchable atlas of your
> family's photographs. It reads each drive once, builds a local catalogue of
> what it found, and leaves every original exactly as it was. Search by what a
> photo shows, who is in it, or roughly when it was taken — and keep searching
> when the drive is sitting in a drawer, unplugged.

## Boilerplate (long)

> AtlasDrive is a private, local-first photo catalogue for macOS. It indexes
> photographs across numbered external drives, storing thumbnails and searchable
> details on your Mac so the whole archive stays browsable when the drives are
> disconnected. Analysis — visual search, face grouping, date estimation — runs
> entirely on-device. Nothing is uploaded, and originals are opened read-only and
> verified unchanged after every batch.

## Voice

AtlasDrive is talking to someone about photographs of people they love. It is
calm, plain and specific, and it never overclaims.

**Do**

- Say what is certain and what is a guess: "Likely between 1985 and 1989."
- Name the next physical action: "Connect Drive 14 to open the original."
- Use everyday words: *photograph*, *drive*, *drawer*, *shelf*.
- Attribute confidence to the software, not the user: "We couldn't read this file."

**Don't**

- Present an estimate as a fact ("Taken in 1987" when it was inferred).
- Use AI-vendor vocabulary in the interface: *embedding*, *vector*, *model*,
  *inference*, *neural*. These belong in the docs and the log, not on screen.
- Say *magic*, *effortless*, *instantly* — the product's honesty is the pitch.
- Blame the user for a failed scan.

### Phrases in the product's own words

| Situation | Wording |
|---|---|
| Uncertain date | Likely between 1985 and 1989 |
| Known date | Taken on 12 August 1998 |
| Scanned print | Scanned in 2022 — original date unknown |
| Offline result | Connect Drive 14 to open the original |
| Visual match | These are visual guesses, not certainties |
| No visual meaning in a query | No visual terms recognised — searched names, folders and tags only |
| Face review | Ready for you to confirm — AtlasDrive never names anyone on its own |

## Palette

Dark-first. Graphite and slate are the map; blue is where you are going; amber is
where something is.

| Token | Name | Hex | Role |
|---|---|---|---|
| `--atlas-graphite` | Atlas Graphite | `#161B22` | Darkest surface; ink on light |
| `--atlas-slate` | Archive Slate | `#252C35` | Raised panels, cards, nav |
| `--atlas-blue` | Navigation Blue | `#3DA9FC` | Primary action, selection, focus |
| `--atlas-amber` | Location Amber | `#F5A623` | Offline, caution, "where it lives" |
| `--atlas-map-white` | Map White | `#F4F6F8` | Light-mode page background |
| `--atlas-clear-white` | Clear White | `#F7F9FB` | Light-mode panels |
| `--atlas-steel` | Steel Grey | `#9AA6B2` | Secondary text, borders, dividers |

### Derived shades

Two shades are derived from the palette rather than added to it, purely to meet
contrast requirements. They are the only permitted additions.

| Token | Hex | Why |
|---|---|---|
| `--atlas-blue-deep` | `#0A63A8` | Navigation Blue as *text* on a light background — the brand blue itself is ~2.4:1 on white and fails WCAG AA |
| `--atlas-amber-deep` | `#8A5A00` | Same reason, for amber status text on light surfaces |

### Usage rules

- **Navigation Blue is a fill, not light-mode body text.** On a blue fill, use
  Atlas Graphite for the label (8.2:1), not white (2.4:1).
- **Amber means location and interruption**, never success. Offline drives,
  "connect this drive", caution states.
- **Green and red are status only** — pass/fail in the verifier and safety
  panels. They are not brand colours and never appear decoratively.
- Body text must clear **4.5:1**; large text and non-text indicators **3:1**.

## Application icon

The mark is a drive plate read as a map: a rounded graphite square, a Navigation
Blue route crossing it, and a single Location Amber point where the route stops.
One idea — *this photograph is here* — legible at 16px.

## Naming things inside the product

- The local database is **the catalogue** (never "the index" in the UI).
- A physical disk is **a drive**, always with its number: *Drive 14*.
- Grouped faces are **people**, and they are only named by the user.
- The safety tool is **the verifier**; what it produces is **a safety check**.
