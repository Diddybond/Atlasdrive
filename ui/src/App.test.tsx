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

  it("shows safety checks on the settings screen", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await waitFor(() => {
      expect(screen.getByText(/network isolation/i)).toBeDefined();
    });
  });
});
