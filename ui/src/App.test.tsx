import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { App } from "./App";

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

  it("lists what is in the photographs and searches when a subject is clicked", async () => {
    render(<App />);
    // The archive is browsable without having to guess a search term.
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /What is in your photographs/ })).toBeDefined();
    });
    // Counts are shown, so you know whether a subject is worth clicking.
    const chip = screen.getByRole("button", { name: /Find 131 photographs of wedding/ });
    expect(chip).toBeDefined();

    fireEvent.click(chip);
    // Clicking is the same operation as typing it, so the box reflects it.
    await waitFor(() => {
      expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("wedding");
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
