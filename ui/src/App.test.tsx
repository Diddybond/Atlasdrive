import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { App } from "./App";

describe("Family Archive UI", () => {
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
      expect(screen.getByText("Family Archive A")).toBeDefined();
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
    expect(screen.getByText(/Drive 14/)).toBeDefined();
  });

  it("shows safety checks on the settings screen", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await waitFor(() => {
      expect(screen.getByText(/network isolation/i)).toBeDefined();
    });
  });
});
