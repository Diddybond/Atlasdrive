# AtlasDrive — demo voiceover script

A walkthrough in one pass, following the sidebar top to bottom. Roughly
**3 minutes 30** at a relaxed pace.

Stage directions are in square brackets. Everything else is read aloud.

Two things to check before recording: the drive counts in the pill row, and
whether Drive 2 is connected. Both are called out below where they matter.

---

## 1. The problem (about 25 seconds)

*[Open on the Search screen. Don't click anything yet.]*

> This is AtlasDrive.
>
> I'm a photographer, and like most photographers I've got a shelf of external
> drives. Weddings, commercial shoots, family photographs, going back years.
>
> The photographs are all there. The problem is finding anything. You end up
> plugging in drive after drive, hoping you picked the right one.
>
> So I built this to fix that.

---

## 2. What it does (about 25 seconds)

*[Still on Search. Gesture at the drive pills along the top.]*

> AtlasDrive reads each drive once and builds a catalogue on my Mac of what it
> found. Thumbnails, dates, who's in the picture, what the picture shows.
>
> Every drive gets a number, matching the label on the physical drive.
>
> Right now that's four drives and about fifteen thousand photographs.

*[Say the real number from the pills — the counts are live.]*

> And here's the part that matters. Most of those drives aren't plugged in.

---

## 3. Search, and the payoff (about 40 seconds)

*[Type "cocktail" and hit Search.]*

> I can search the whole archive whether the drives are connected or not.

*[Point at the result card.]*

> It tells me what it found and, more importantly, which numbered drive it's on.
> So I know exactly which one to pull off the shelf.

*[Scroll to the subject list.]*

> This is everything it recognised across the drives. I can click any of these
> to narrow things down, and stack them up to get more specific.

*[Click one or two subjects.]*

> None of this needed the drives connected. It's all read from the local
> catalogue.

---

## 4. The honesty bit (about 25 seconds)

*[Point at the line under the subject list.]*

> One thing I was careful about. When AtlasDrive gives you a name, that name
> came from text it actually read in the photograph. A label on a bottle, a shop
> front, a van, a magazine.
>
> It never guesses a name from the image itself.
>
> Same with dates. If it isn't certain, it says "likely between these years"
> rather than pretending it knows. I'd rather it told me what it isn't sure
> about.

---

## 5. Drives (about 30 seconds)

*[Click Drives.]*

> This is where a drive gets added. You point it at the drive, give it the
> number that's on the label, and note where it physically lives. Drawer two,
> studio shelf B, whatever makes sense to you.

*[Point at an existing drive.]*

> Once it's indexed, it stays searchable forever, whether it's connected or in
> a drawer.
>
> And I can ask it to check an existing drive for new photographs without
> reading the whole thing again.

---

## 6. Scan activity (about 20 seconds)

*[Click Scan activity.]*

> This is the indexing itself. It shows what it's working through and how far
> along it is.
>
> All of it runs on this machine. Nothing gets uploaded, and the indexing path
> makes no network calls at all. That was a hard rule from the start — these are
> photographs of people's weddings and people's families.

---

## 7. People (about 25 seconds)

*[Click People.]*

> AtlasDrive groups faces it finds across every drive, and I put names to them.

*[Show a named person, or the empty state if there isn't one yet.]*

> Once someone's named, I can find every photograph of them across the whole
> archive in one go.
>
> Face data is encrypted on disk. It never leaves the Mac.

---

## 8. Events (about 25 seconds)

*[Click Events.]*

> Photographs naturally cluster in time, so AtlasDrive groups them into events
> and I name them. A wedding, a shoot, a client job.

*[Show a named event.]*

> If it's grouped two jobs into one, I can split them. If it's split one job in
> two, I can fold them back together.

---

## 9. Safety, and the close (about 40 seconds)

*[Click Settings.]*

> Last thing, and it's the reason I built it this way.
>
> AtlasDrive opens every original read-only. It never deletes, moves, renames or
> rewrites anything. After each batch it checks the originals haven't changed,
> and if anything has been altered it stops immediately.
>
> It's a catalogue. It doesn't touch your photographs.

*[Pause.]*

> It's built in Rust, with a Tauri shell and a React and TypeScript interface.
> The catalogue is SQLite. The image analysis runs on-device.
>
> The whole thing is one app on one Mac, with no account, no subscription and no
> cloud.
>
> A shelf of drives, on one map.

---

## Notes

**If you want a shorter cut (about 90 seconds):** keep sections 1, 3 and 9.
That's the problem, the payoff and the promise, which is the whole pitch.

**Strongest single moment:** searching with the drives unplugged, and it still
finding the photograph and naming the drive. Give that a beat of silence before
you explain it.

**Worth avoiding on camera:** words like *embedding*, *vector*, *model* and
*inference*. They're accurate but they make it sound like an AI demo rather
than a tool that finds your photographs. The brand doc rules them out of the
interface for the same reason.
