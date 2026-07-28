import { setMockScanError, setMockScanning, setMockStopping } from "./api";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { App } from "./App";

// The mock backend keeps state between calls on purpose — a scan that advances
// is what makes the rate and finish-time working testable. That state has to be
// reset, or a test that turns the scan off silently changes what every later
// test is looking at.
beforeEach(() => {
  cleanup();
  setMockScanning(true);
  setMockStopping(false);
  setMockScanError(null);
});

describe("AtlasDrive UI", () => {
  it("renders the main navigation and search screen", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: "Search", level: 1 })).toBeDefined();
    expect(screen.getByRole("search")).toBeDefined();
  });

  it("navigates to Drives and lists numbered drives", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Drives", level: 1 })).toBeDefined();
    });
    // Mock data includes Drive 14.
    await waitFor(() => {
      expect(screen.getByText("AtlasDrive A")).toBeDefined();
    });
  });

  it("runs an offline-aware search and shows drive + status on results", async () => {
    render(<App />);
    const input = screen.getByRole("searchbox");
    fireEvent.change(input, { target: { value: "beach" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText("beach_1998.jpg")).toBeDefined();
    });
    // The drive badge on the result card. "Drive 14" also appears in the
    // which-drive banner, so this has to be the badge specifically.
    expect(screen.getAllByText("Drive 14").length).toBeGreaterThan(0);
  });

  it("says which drive to connect before listing the photographs", async () => {
    render(<App />);
    // "visual" matches every mock result, which spans three drives — one
    // connected, two not.
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "visual" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));

    await waitFor(() => {
      expect(screen.getByText(/Found on Drives 7, 14 and 22/)).toBeDefined();
    });
    // The disconnected drives are named with where they are kept, so the user
    // knows which physical disk to go and fetch.
    expect(screen.getByText(/Connect Drive 7 \(Drawer 2\), Drive 22 \(Box A\)/)).toBeDefined();
    // Per-drive counts are shown.
    expect(screen.getAllByText(/1 photograph$/).length).toBeGreaterThan(0);
  });

  it("browses a subject without disturbing the search box", async () => {
    render(<App />);
    // The archive is browsable without having to guess a search term.
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /What is in your photographs/ })).toBeDefined();
    });
    const chip = screen.getByRole("button", { name: /Narrow to the 131 photographs showing wedding/ });
    fireEvent.click(chip);

    // Results arrive from the tag itself — no text search involved, and the
    // box stays the owner's own. Writing the tag into it was how "jeans"
    // ended up answered through the lens of a stale "likely-scan".
    await waitFor(() => {
      expect(screen.getByText("beach_1998.jpg")).toBeDefined();
    });
    expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("");
    expect(
      screen.getByRole("button", { name: /Stop narrowing to wedding/ }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  /// The exact failure the owner hit: a hyphenated subject with hundreds of
  /// photographs behind it must browse to them, not to an empty page.
  it("a hyphenated subject still finds its photographs", async () => {
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /What is in your photographs/ })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /showing likely-scan/ }));
    await waitFor(() => {
      expect(screen.getByText("old_scan.jpg")).toBeDefined();
    });
  });

  /// Two subjects must mean "both", not "either" — the whole point of picking
  /// a second one is to see fewer photographs, not more.
  it("says plainly that picking two subjects requires both", async () => {
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /What is in your photographs/ })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /showing wedding/ }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Stop narrowing to wedding/ })).toBeDefined();
    });
    const second = screen.getAllByRole("button", { name: /Narrow to the/ })[0];
    fireEvent.click(second);
    await waitFor(() => {
      // The sentence is split by its own emphasis, so match the lead-in.
      expect(screen.getByText(/Showing only photographs that contain/i)).toBeDefined();
    });
    await waitFor(() => {
      expect(screen.getByText(/^all$/)).toBeDefined();
    });
  });

  /// Choosing a drive must not carry subjects across to it: the new drive may
  /// not have them, and an empty result with no visible cause reads as a bug.
  it("clears picked subjects when the drive changes", async () => {
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /What is in your photographs/ })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /showing wedding/ }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Stop narrowing to wedding/ })).toBeDefined();
    });

    fireEvent.click(screen.getByRole("button", { name: /^Drive 2/ }));
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /Stop narrowing to/ })).toBeNull();
    });
  });

  it("explains which visual terms the local encoder understood", async () => {
    render(<App />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "beach" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText(/appear to show: beach/i)).toBeDefined();
    });
    // Visual matches must never be presented as certainties.
    expect(screen.getByText(/visual guesses/i)).toBeDefined();
  });

  it("says so plainly when a query carries no visual meaning", async () => {
    render(<App />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "zzzz" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText(/No visual terms recognised/i)).toBeDefined();
    });
  });

  it("offers Show in Finder only for connected drives", async () => {
    render(<App />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "beach" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText("beach_1998.jpg")).toBeDefined();
    });
    // beach_1998.jpg sits on a connected drive, so it can be revealed.
    expect(screen.getByRole("button", { name: /Show beach_1998.jpg in Finder/ })).toBeDefined();

    // portrait.jpg is on Drive 7, which is disconnected: no button, and the
    // card says which drive to plug in instead.
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "portrait" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText("portrait.jpg")).toBeDefined();
    });
    expect(screen.queryByRole("button", { name: /Show portrait.jpg in Finder/ })).toBeNull();
    expect(screen.getByText(/Connect Drive 7 to open the original/)).toBeDefined();
  });

  it("exports a diagnostics file and says what it does not contain", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await waitFor(() => {
      expect(screen.getByText(/never your file names/i)).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /Create diagnostics file/ }));
    await waitFor(() => {
      expect(screen.getByText(/diagnostics-sample.json/)).toBeDefined();
    });
  });

  it("shows and edits where a drive is kept and what is on it", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText("AtlasDrive A")).toBeDefined();
    });
    expect(screen.getByText(/What's on it: family, holidays/)).toBeDefined();

    fireEvent.click(
      screen.getByRole("button", { name: /Edit location and categories for Drive 14/ }),
    );
    fireEvent.change(screen.getByLabelText(/Where this drive is kept/), {
      target: { value: "Loft box 3" },
    });
    fireEvent.change(screen.getByLabelText(/What's on it/), {
      target: { value: "negatives, weddings" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(screen.getByText(/Loft box 3/)).toBeDefined();
    });
    expect(screen.getByText(/What's on it: negatives, weddings/)).toBeDefined();
  });

  it("lets the user correct a date and refuses a malformed one", async () => {
    render(<App />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "beach" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText("beach_1998.jpg")).toBeDefined();
    });

    fireEvent.click(screen.getByRole("button", { name: /Correct the date for beach_1998.jpg/ }));

    // A malformed date is refused in plain language, not silently accepted.
    fireEvent.change(screen.getByLabelText(/Date taken/), { target: { value: "12/08/1998" } });
    fireEvent.click(screen.getByRole("button", { name: "Save date" }));
    await waitFor(() => {
      expect(screen.getByText(/enter the date as YYYY-MM-DD/i)).toBeDefined();
    });

    // A valid one is accepted and shown back.
    fireEvent.change(screen.getByLabelText(/Date taken/), { target: { value: "1998-08-12" } });
    fireEvent.click(screen.getByRole("button", { name: "Save date" }));
    await waitFor(() => {
      expect(screen.getByText("Taken on 1998-08-12")).toBeDefined();
    });
  });

  it("browses faces as pictures and names one without knowing it first", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /People/ }));

    // The gallery is pictures, not names — you can browse before knowing anyone.
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /Unnamed face/ }).length).toBeGreaterThan(0);
    });

    fireEvent.click(screen.getAllByRole("button", { name: /Unnamed face, 34 photographs/ })[0]);
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Name this face" })).toBeDefined();
    });

    // Nothing can be saved until a name is typed — nothing is named automatically.
    expect((screen.getByRole("button", { name: "Save name" }) as HTMLButtonElement).disabled).toBe(
      true,
    );

    fireEvent.change(screen.getByLabelText(/Who is this/), { target: { value: "Aimee" } });
    fireEvent.click(screen.getByRole("button", { name: "Save name" }));

    await waitFor(() => {
      expect(screen.getByText(/Tagged as Aimee/)).toBeDefined();
    });
    // Naming one face offers the others it thinks are the same person, rather
    // than silently attaching them.
    expect(screen.getByText(/might also be Aimee/)).toBeDefined();
    expect(screen.getByText(/34 photographs/)).toBeDefined();
    expect(screen.getByRole("button", { name: /Review 2 possible matches for Aimee/ })).toBeDefined();
  });

  it("gathers a named person's photographs and says which drive is missing", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /People/ }));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /Unnamed face/ }).length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getAllByRole("button", { name: /Unnamed face/ })[0]);
    fireEvent.change(screen.getByLabelText(/Who is this/), { target: { value: "Kent" } });
    fireEvent.click(screen.getByRole("button", { name: "Save name" }));

    // Per-person actions live behind Manage, so the row stays readable.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /More actions for Kent/ })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /More actions for Kent/ }));
    fireEvent.change(screen.getByLabelText(/Copy their photographs into/), {
      target: { value: "/Users/wayne/Desktop/Kent" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Gather Kent's photographs/ }));

    await waitFor(() => {
      expect(screen.getByText(/Copied 1 photograph to/)).toBeDefined();
    });
    // The photographs it could not reach are attributed to a specific drive.
    expect(screen.getByText(/1 more are on Drive 7/)).toBeDefined();
  });

  it("shows where a person's photographs are and offers to open the folder", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /People/ }));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /Unnamed face/ }).length).toBeGreaterThan(0);
    });
    fireEvent.click(screen.getAllByRole("button", { name: /Unnamed face/ })[0]);
    fireEvent.change(screen.getByLabelText(/Who is this/), { target: { value: "Margaret" } });
    fireEvent.click(screen.getByRole("button", { name: "Save name" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /More actions for Margaret/ })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: /More actions for Margaret/ }));
    fireEvent.click(screen.getByRole("button", { name: /Show where Margaret's/ }));

    await waitFor(() => {
      expect(screen.getByText("Aimee and Kent/edits")).toBeDefined();
    });
    // A connected drive can be opened; a disconnected one says what to plug in.
    expect(screen.getByRole("button", { name: /Open Aimee and Kent\/edits in Finder/ })).toBeDefined();
    expect(screen.getByText(/Connect Drive 7 to open/)).toBeDefined();
  });

  it("removes a person added by mistake without losing the faces", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /People/ }));

    // Operate on whoever is listed first rather than a name this test created —
    // earlier tests share the mock's state, so the gallery may be fully named.
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /More actions for / }).length).toBeGreaterThan(0);
    });
    const manage = screen.getAllByRole("button", { name: /More actions for / })[0];
    const person = (manage.getAttribute("aria-label") ?? "").replace(/^More actions for /, "");
    const before = screen.getAllByRole("button", { name: /More actions for / }).length;

    fireEvent.click(manage);
    fireEvent.click(screen.getByRole("button", { name: `Remove ${person}` }));

    await waitFor(() => {
      expect(screen.getByText(/faces are kept and are unnamed again/)).toBeDefined();
    });
    expect(screen.getByText(new RegExp(`Removed ${person}`))).toBeDefined();
    // One fewer person, and the faces themselves are untouched.
    expect(screen.queryAllByRole("button", { name: /More actions for / }).length).toBe(before - 1);
  });

  it("offers to check a drive for photographs added since the last scan", async () => {
    setMockScanning(false);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText("AtlasDrive A")).toBeDefined();
    });
    fireEvent.click(
      screen.getByRole("button", { name: /Check Drive 14 for new photographs/ }),
    );
    await waitFor(() => {
      expect(screen.getByText(/Looking for new photographs on Drive 14/)).toBeDefined();
    });
  });

  it("shows safety checks on the settings screen", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await waitFor(() => {
      expect(screen.getByText(/network isolation/i)).toBeDefined();
    });
  });
});

describe("Backup", () => {
  async function openSettings() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Backup", level: 2 })).toBeDefined();
    });
  }

  it("cannot back up until a folder has been chosen", async () => {
    await openSettings();
    const button = screen.getByRole("button", { name: "Back up now" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(screen.getByText("not chosen yet")).toBeDefined();
  });

  it("says plainly whether a backup will leave the Mac", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: /Choose|Change/ }));
    // The mock picker returns a Google Drive path.
    await waitFor(() => {
      expect(screen.getByText(/synchronised by Google Drive/)).toBeDefined();
    });
    expect(screen.getByText(/never connects to the internet/)).toBeDefined();
  });

  it("backs up and then lists the backup it made", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: /Choose|Change/ }));
    await waitFor(() => {
      expect((screen.getByRole("button", { name: "Back up now" }) as HTMLButtonElement).disabled)
        .toBe(false);
    });

    fireEvent.click(screen.getByRole("button", { name: "Back up now" }));
    await waitFor(() => {
      expect(screen.getByText(/new thumbnails/)).toBeDefined();
    });
    // Only the genuinely new thumbnails are copied — that is what makes a
    // nightly cloud backup affordable.
    expect(screen.getByText(/14478 already there/)).toBeDefined();
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Available backups", level: 3 })).toBeDefined();
    });
  });

  it("asks before replacing the catalogue, and says the old one is kept", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: /Choose|Change/ }));
    await waitFor(() => {
      expect((screen.getByRole("button", { name: "Back up now" }) as HTMLButtonElement).disabled)
        .toBe(false);
    });
    fireEvent.click(screen.getByRole("button", { name: "Back up now" }));
    // Several backups may be listed; the newest is first.
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /Restore…/ }).length).toBeGreaterThan(0);
    });

    // Restoring is destructive enough to need a second click.
    fireEvent.click(screen.getAllByRole("button", { name: /Restore…/ })[0]);
    await waitFor(() => {
      expect(screen.getByRole("alert")).toBeDefined();
    });
    expect(screen.getByText(/kept on disk, not deleted/)).toBeDefined();

    fireEvent.click(screen.getByRole("button", { name: /Yes, replace it/ }));
    await waitFor(() => {
      expect(screen.getByText(/named people/)).toBeDefined();
    });
  });
});

describe("Events", () => {
  async function openEvents() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Events/ }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Events", level: 1 })).toBeDefined();
    });
  }

  it("starts with nothing and offers to find events", async () => {
    await openEvents();
    expect(screen.getByRole("button", { name: "Find events" })).toBeDefined();
    expect(screen.getByText(/None yet/)).toBeDefined();
  });

  it("reviews one proposal at a time rather than showing a wall", async () => {
    await openEvents();
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Name this one", level: 2 })).toBeDefined();
    });
    // Two were proposed, but only one is being asked about.
    expect(screen.getByText("2 waiting")).toBeDefined();
    expect(screen.getAllByRole("heading", { name: "Name this one" })).toHaveLength(1);
    // An unnamed proposal still describes itself by its dates.
    expect(screen.getByText(/758 photographs/)).toBeDefined();
  });

  it("names an event with a client and moves to the next", async () => {
    await openEvents();
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Name this one", level: 2 })).toBeDefined();
    });

    fireEvent.change(screen.getByLabelText(/What was it/), {
      target: { value: "Aimee & Kent wedding" },
    });
    fireEvent.change(screen.getByLabelText(/Client/), { target: { value: "Aimee Kanovan" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    // It appears under named events, with its client...
    await waitFor(() => {
      expect(screen.getByText("Aimee & Kent wedding")).toBeDefined();
    });
    // The client shows in two places by design: on the event row, and in the
    // Clients list that gathers several shoots for the same people.
    expect(screen.getAllByText(/Aimee Kanovan/).length).toBeGreaterThanOrEqual(2);
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Clients", level: 2 })).toBeDefined();
    });
    // ...and the next proposal is now the one being asked about.
    await waitFor(() => {
      expect(screen.getByText("1 waiting")).toBeDefined();
    });
  });

  it("cannot save an event without a name", async () => {
    await openEvents();
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeDefined();
    });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("can reject a proposal that is not really an event", async () => {
    await openEvents();
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));
    await waitFor(() => {
      expect(screen.getByText("2 waiting")).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: "Not an event" }));
    await waitFor(() => {
      expect(screen.getByText("1 waiting")).toBeDefined();
    });
  });
});

describe("Searching within an event", () => {
  async function namedEvent() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Events/ }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Find events" })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeDefined();
    });
    fireEvent.change(screen.getByLabelText(/What was it/), {
      target: { value: "Crown Parents 2026" },
    });
    fireEvent.change(screen.getByLabelText(/Client/), { target: { value: "Crown School" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(screen.getByText("Crown Parents 2026")).toBeDefined();
    });
  }

  it("jumps to Search scoped to the event, and says so", async () => {
    await namedEvent();
    fireEvent.click(screen.getByRole("button", { name: "Show photographs" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Search", level: 1 })).toBeDefined();
    });
    // The scope has to be visible, or a short list reads as a small archive.
    const scope = await screen.findByRole("status", { name: "Search scope" });
    expect(scope.textContent).toContain("Crown Parents 2026");
    expect(screen.getByRole("button", { name: /Search everything instead/ })).toBeDefined();
  });

  it("can drop the scope and search the whole archive again", async () => {
    await namedEvent();
    fireEvent.click(screen.getByRole("button", { name: "Show photographs" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /Search everything instead/ })).toBeDefined();
    });

    fireEvent.click(screen.getByRole("button", { name: /Search everything instead/ }));
    await waitFor(() => {
      expect(screen.queryByText(/Searching within/)).toBeNull();
    });
  });

  it("a client chip scopes to every shoot for those people", async () => {
    await namedEvent();
    fireEvent.click(screen.getByRole("button", { name: /Crown School/ }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Search", level: 1 })).toBeDefined();
    });
    const scope = await screen.findByRole("status", { name: "Search scope" });
    expect(scope.textContent).toContain("everything for Crown School");
  });
});

describe("More like this", () => {
  it("finds visually similar photographs and says that is what it did", async () => {
    render(<App />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "beach" } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(screen.getByText("beach_1998.jpg")).toBeDefined();
    });

    fireEvent.click(
      screen.getByRole("button", { name: /look like beach_1998\.jpg/ }),
    );

    // The banner must say these are visual matches, not text matches — the
    // distinction is the whole point of the feature.
    const scope = await screen.findByRole("status", { name: "Result scope" });
    expect(scope.textContent).toContain("look like");
    expect(scope.textContent).toContain("beach_1998.jpg");
    // The photograph itself is not among its own results — it appears once, in
    // the banner naming what was asked for, and not again in the list below.
    expect(screen.getAllByText("beach_1998.jpg")).toHaveLength(1);
  });
});

describe("Adjusting an event", () => {
  async function namedWedding() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Events/ }));
    await waitFor(() => screen.getByRole("button", { name: "Find events" }));
    fireEvent.click(screen.getByRole("button", { name: "Find events" }));
    await waitFor(() => screen.getByRole("button", { name: "Save" }));
    fireEvent.change(screen.getByLabelText(/What was it/), {
      target: { value: "Wedding day" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => screen.getByText("Wedding day"));
  }

  it("offers the natural break rather than asking for a timestamp", async () => {
    await namedWedding();
    fireEvent.click(screen.getAllByRole("button", { name: "Adjust…" })[0]);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Split it in two", level: 3 })).toBeDefined();
    });
    // Described as a pause, with the consequence stated — no timestamps typed.
    expect(screen.getByText(/3\.5-hour pause/)).toBeDefined();
    expect(screen.getByText(/210 photographs would move/)).toBeDefined();
  });

  it("splits an event at the offered break", async () => {
    await namedWedding();
    fireEvent.click(screen.getAllByRole("button", { name: "Adjust…" })[0]);
    await waitFor(() => screen.getByRole("button", { name: "Split here" }));

    fireEvent.click(screen.getByRole("button", { name: "Split here" }));
    await waitFor(() => {
      expect(screen.getByText(/now their own event/)).toBeDefined();
    });
  });

  it("merges another event into this one", async () => {
    await namedWedding();
    fireEvent.click(screen.getAllByRole("button", { name: "Adjust…" })[0]);
    await waitFor(() => screen.getByLabelText("Event to fold in"));

    const select = screen.getByLabelText("Event to fold in") as HTMLSelectElement;
    // The event being adjusted must not be offered as something to fold in.
    const options = [...select.options].map((o) => o.value).filter(Boolean);
    expect(options.length).toBeGreaterThan(0);
    fireEvent.change(select, { target: { value: options[0] } });
    fireEvent.click(screen.getByRole("button", { name: "Merge in" }));

    await waitFor(() => {
      expect(screen.getByText(/Merged \d+ photograph/)).toBeDefined();
    });
  });
});

describe("Knowing when a drive can be unplugged", () => {
  it("says a finished drive is safe to unplug", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText(/Finished — all 4,213 photographs indexed/)).toBeDefined();
    });
    // Exactly one mock drive is finished. The others -- one part-indexed, one
    // never scanned -- must not claim to be.
    expect(screen.getAllByText(/Safe to unplug/)).toHaveLength(1);
  });

  it("never calls a drive that has not been scanned safe to unplug", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText(/Never indexed/)).toBeDefined();
    });
    const row = screen.getByText(/Never indexed/);
    expect(row.textContent).not.toContain("Safe to unplug");
    // And it is styled as outstanding work, not as done.
    expect(row.className).toContain("working");
  });

  it("says an unfinished drive must stay connected, and never says safe", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText(/4,000 still to do/)).toBeDefined();
    });
    const row = screen.getByText(/4,000 still to do/);
    expect(row.textContent).toContain("leave this drive connected");
    // The reassuring phrase must never appear on unfinished work.
    expect(row.textContent).not.toContain("Safe to unplug");
    expect(row.textContent).toContain("73%");
  });
});

describe("Picking a drive to register", () => {
  async function openRegister() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => screen.getByRole("button", { name: /Register a drive/ }));
    fireEvent.click(screen.getByRole("button", { name: /Register a drive/ }));
    await waitFor(() => screen.getByLabelText(/Which drive/));
  }

  it("offers the drives that are actually plugged in", async () => {
    await openRegister();
    const select = screen.getByLabelText(/Which drive/) as HTMLSelectElement;
    const labels = [...select.options].map((o) => o.text);
    expect(labels).toContain("Late 25 B");
    expect(labels.some((l) => l.startsWith("New Volume"))).toBe(true);
  });

  it("will not let you register the same disk twice", async () => {
    await openRegister();
    const select = screen.getByLabelText(/Which drive/) as HTMLSelectElement;
    const taken = [...select.options].find((o) => o.text.includes("already Drive 1"));
    expect(taken).toBeDefined();
    expect(taken!.disabled).toBe(true);
  });

  it("marks the startup disk rather than hiding it", async () => {
    await openRegister();
    const select = screen.getByLabelText(/Which drive/) as HTMLSelectElement;
    const boot = [...select.options].find((o) => o.text.includes("startup disk"));
    // Offered, because someone's photographs may genuinely live there...
    expect(boot).toBeDefined();
    // ...but not selectable by accident without being told what it is.
    expect(boot!.disabled).toBe(false);
  });

  it("fills the name from the disk and suggests where photographs live", async () => {
    await openRegister();
    fireEvent.change(screen.getByLabelText(/Which drive/), {
      target: { value: "/Volumes/Late 25 B" },
    });

    await waitFor(() => {
      expect((screen.getByLabelText(/Folder to index/) as HTMLInputElement).value).toBe(
        "/Volumes/Late 25 B",
      );
    });
    // The disk's own label is a better default than an empty box.
    await waitFor(() => {
      const name = screen.getByPlaceholderText("e.g. AtlasDrive A") as HTMLInputElement;
      expect(name.value).toBe("Late 25 B");
    });
    // And the folders on it where photographs usually are.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Photos" })).toBeDefined();
    });
    fireEvent.click(screen.getByRole("button", { name: "Weddings" }));
    expect((screen.getByLabelText(/Folder to index/) as HTMLInputElement).value).toBe(
      "/Volumes/Late 25 B/Weddings",
    );
  });
});

describe("Read-only drives", () => {
  async function openRegister() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => screen.getByRole("button", { name: /Register a drive/ }));
    fireEvent.click(screen.getByRole("button", { name: /Register a drive/ }));
    await waitFor(() => screen.getByLabelText(/Which drive/));
  }

  it("marks a read-only drive in the list", async () => {
    await openRegister();
    const select = screen.getByLabelText(/Which drive/) as HTMLSelectElement;
    const ro = [...select.options].find((o) => o.text.includes("read-only"));
    expect(ro).toBeDefined();
    // Read-only is normal for this app, so it must still be selectable.
    expect(ro!.disabled).toBe(false);
  });

  it("explains rather than fails when the drive cannot be written to", async () => {
    await openRegister();
    fireEvent.change(screen.getByLabelText(/Which drive/), {
      target: { value: "/Volumes/New Volume" },
    });

    await waitFor(() => {
      expect(screen.getByText(/exactly how AtlasDrive reads your photographs/)).toBeDefined();
    });
    // The identity-file option is off and unavailable, not left to fail.
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    const identity = boxes.find((b) => b.disabled);
    expect(identity).toBeDefined();
    expect(identity!.checked).toBe(false);
  });
});

describe("Stopping a scan", () => {
  /// A scan runs for days. The owner has to be able to end it and put a
  /// different drive on instead, without waiting for it or resorting to the
  /// command line.
  it("offers a stop button while a scan is running", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Scan activity/ }));
    const stop = await screen.findByRole("button", { name: /Stop scanning/ });
    expect(stop).toBeDefined();

    fireEvent.click(stop);
    await waitFor(() => {
      expect(screen.getByText(/Stopping after the current batch/i)).toBeDefined();
    });
    // And it must not invite a second press while the first is being obeyed.
    expect(
      (screen.getByRole("button", { name: /Stopping/ }) as HTMLButtonElement).disabled,
    ).toBe(true);
    setMockStopping(false);
  });

  /// Nothing about stopping should suggest work has been thrown away.
  it("says plainly that stopping loses nothing", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Scan activity/ }));
    fireEvent.click(await screen.findByRole("button", { name: /Stop scanning/ }));
    await waitFor(() => {
      expect(screen.getByText(/Interrupting loses nothing/i)).toBeDefined();
    });
    setMockStopping(false);
  });
});

describe("Folder stories on the Drives screen", () => {
  /// The owner's ask, near-verbatim: "This folder appears to contain an event
  /// for Crown Paints." One button per drive, one sentence per folder.
  it("tells the story of each folder in plain language", async () => {
    setMockScanning(false);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    const btn = (await screen.findAllByRole("button", { name: /What is in each folder/ }))[0];
    fireEvent.click(btn);

    await waitFor(() => {
      expect(screen.getByText("Crown 6th July 2025")).toBeDefined();
    });
    expect(screen.getByText(/Looks like an event/)).toBeDefined();
    expect(screen.getByText(/crown-paints/)).toBeDefined();
    // And the guess always carries its evidence, so it can be judged.
    expect(screen.getByText(/132 photos/)).toBeDefined();
  });
});

describe("Managing scans from the Drives screen", () => {
  /// The owner's requirement, stated plainly: stop a scan, come back to that
  /// drive later, or put a different drive on — all without a command line.
  it("offers to stop the drive that is being scanned", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    const stop = await screen.findByRole("button", { name: /Stop scanning Drive 14/ });
    fireEvent.click(stop);
    await waitFor(() => {
      expect(screen.getByText(/Stopping after the current batch/i)).toBeDefined();
    });
    setMockStopping(false);
  });

  /// While one drive is being read, another must not be startable behind its
  /// back — and the screen has to say which drive is holding things up rather
  /// than presenting a dead button.
  it("says which drive is busy instead of offering a second scan", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getAllByRole("button", { name: /Drive 14 is scanning/ }).length).toBeGreaterThan(
        0,
      );
    });
    const busy = screen.getAllByRole("button", { name: /Drive 14 is scanning/ })[0];
    expect((busy as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("Scanning a drive from the Drives screen", () => {
  async function openDrives() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Drives/ }));
    await waitFor(() => {
      expect(screen.getByText(/Never indexed/)).toBeDefined();
    });
  }

  it("offers to scan a drive that has never been indexed", async () => {
    setMockScanning(false);
    await openDrives();
    // The never-scanned drive must offer a first scan, not a check for new
    // photographs -- and must not send you to another screen to do it.
    expect(screen.getByRole("button", { name: /^Scan Drive 22$/ })).toBeDefined();
  });

  it("offers to check an already-indexed drive for new photographs", async () => {
    setMockScanning(false);
    await openDrives();
    expect(
      screen.getByRole("button", { name: /Check Drive 14 for new photographs/ }),
    ).toBeDefined();
  });

  /// The silence that cost a day: the owner pressed "Check for new
  /// photographs", the note said it was looking, the background run died
  /// immediately, and nothing ever contradicted the note.
  it("says so when a scan dies instead of leaving the note on screen", async () => {
    setMockScanError("/Volumes/Late 25 A is not available.");
    setMockScanning(false);
    try {
      await openDrives();
      fireEvent.click(screen.getByRole("button", { name: /^Scan Drive 22$/ }));
      await waitFor(
        () => {
          expect(screen.getByText(/stopped before it started properly/i)).toBeDefined();
        },
        { timeout: 4000 },
      );
      // And the reason has to be the real one, not a generic apology.
      expect(screen.getByText(/is not available/)).toBeDefined();
    } finally {
      setMockScanError(null);
    }
  }, 10000);

  it("shows the outcome against the drive it concerns", async () => {
    setMockScanning(false);
    await openDrives();
    fireEvent.click(screen.getByRole("button", { name: /^Scan Drive 22$/ }));

    await waitFor(() => {
      expect(screen.getByText(/Looking for new photographs on Drive 22/)).toBeDefined();
    });
    // Inside that drive's card, not floating at the top of the page.
    const note = screen.getByText(/Looking for new photographs on Drive 22/);
    expect(note.closest(".drive-card")).not.toBeNull();
    const card = note.closest(".drive-card")!;
    expect(card.textContent).toContain("Scanned prints");
  });
});

describe("Scan progress dashboard", () => {
  async function openScan() {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Scan activity/ }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Scan activity", level: 1 })).toBeDefined();
    });
  }

  it("shows how far through the drive it is", async () => {
    await openScan();
    await waitFor(() => {
      expect(screen.getByText(/8,333 photographs/)).toBeDefined();
    });
    // A progress bar carrying a real value, not a decorative strip.
    const bar = screen.getByRole("progressbar");
    expect(Number(bar.getAttribute("aria-valuenow"))).toBeGreaterThanOrEqual(0);
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    // And the headline counts, as tiles.
    expect(screen.getByText("Photographs found")).toBeDefined();
    expect(screen.getByText("Left to read")).toBeDefined();
  });

  it("says it is safe to walk away", async () => {
    await openScan();
    await waitFor(() => {
      expect(screen.getByText(/stays awake while scanning/)).toBeDefined();
    });
    expect(screen.getByText(/Interrupting loses nothing/)).toBeDefined();
    expect(screen.getByText(/read-only/)).toBeDefined();
  });

  it("shows a live feed of the photographs it has just read", async () => {
    await openScan();
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Live feed", level: 3 })).toBeDefined();
    });
    const rows = document.querySelectorAll(".feed li");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows[0].textContent).toMatch(/IMG_\d+\.jpg/);
    // Alongside the running totals of what has been recognised.
    expect(screen.getByRole("heading", { name: "What it has found", level: 3 })).toBeDefined();
    expect(screen.getByText("Faces")).toBeDefined();
  });

  it("waits for real evidence before quoting a speed", async () => {
    await openScan();
    // One sample proves nothing, so the gauge must say so rather than divide by
    // a near-zero interval and swing to a wild figure.
    await waitFor(() => {
      expect(screen.getByText("Measuring…")).toBeDefined();
    });
    const gauge = screen.getByRole("img", { name: /Read speed/ });
    expect(gauge.getAttribute("aria-label")).toContain("not yet measured");
  });

  it("scales the gauge so a slow drive still moves the needle", async () => {
    await openScan();
    await waitFor(() => {
      expect(screen.getByRole("img", { name: /Read speed/ })).toBeDefined();
    });
    // The dial is drawn from the data, so a fixed 0-1000 face never appears.
    const bounds = [...document.querySelectorAll(".gauge-bound")].map((n) => n.textContent);
    expect(bounds[0]).toBe("0");
    expect(Number(bounds[1])).toBeLessThanOrEqual(2000);
  });
});

