import { describe, expect, it } from "vitest";
import { byName } from "../api";

describe("alphabetical order", () => {
  it("ignores case and sorts numbers as numbers", () => {
    const names = ["sweet dreams beds", "Ashbury office shots", "ribble cycles", "Drive 10", "Drive 2", "Simplex"];
    expect([...names].sort(byName)).toEqual([
      "Ashbury office shots",
      "Drive 2",
      "Drive 10",
      "ribble cycles",
      "Simplex",
      "sweet dreams beds",
    ]);
  });
});
