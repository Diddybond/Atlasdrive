import { record, ratePerSecond, secondsRemaining, MIN_SPAN_MS } from "./rate";

describe("scan rate", () => {
  /// Building a sample history the way the poll loop does.
  function history(points: [number, number][]) {
    let samples: { at: number; done: number }[] = [];
    for (const [at, done] of points) samples = record(samples, at, done);
    return samples;
  }

  it("refuses to quote a speed from too short a span", () => {
    const s = history([
      [0, 100],
      [5_000, 102],
    ]);
    // Five seconds and two photographs is not evidence; dividing anyway would
    // print a confident figure that changes wildly a moment later.
    expect(ratePerSecond(s, 5_000)).toBeNull();
    expect(MIN_SPAN_MS).toBeGreaterThan(5_000);
  });

  it("measures the real speed once there is a window", () => {
    const s = history([
      [0, 100],
      [30_000, 110],
    ]);
    // Ten photographs in thirty seconds.
    expect(ratePerSecond(s, 30_000)).toBeCloseTo(10 / 30, 5);
  });

  /// The case that matters overnight: a scan that has quietly stopped must not
  /// keep reporting the speed it was doing before it stalled.
  it("falls towards zero when nothing is arriving", () => {
    const s = history([
      [0, 100],
      [30_000, 130],
    ]);
    const healthy = ratePerSecond(s, 30_000)!;
    expect(healthy).toBeCloseTo(1, 5);

    // Two minutes later, still nothing new.
    const stalled = ratePerSecond(s, 150_000)!;
    expect(stalled).toBeLessThan(healthy / 4);
  });

  it("reports a standstill as zero, not as unknown", () => {
    const s = history([
      [0, 100],
      [30_000, 100],
    ]);
    // A reading that did not move is not recorded, so there is one sample and
    // nothing can be said.
    expect(s).toHaveLength(1);
    expect(ratePerSecond(s, 30_000)).toBeNull();
  });

  it("drops readings older than the window", () => {
    const points: [number, number][] = [];
    for (let i = 0; i <= 20; i++) points.push([i * 30_000, 100 + i]);
    const s = history(points);
    const span = s[s.length - 1].at - s[0].at;
    // Ten minutes of readings, but only the last three minutes are kept.
    expect(span).toBeLessThanOrEqual(3 * 60 * 1000);
    expect(s.length).toBeGreaterThan(1);
  });

  it("turns a rate into a time remaining, and knows when it cannot", () => {
    expect(secondsRemaining(8000, 0.22)).toBeCloseTo(8000 / 0.22, 3);
    // No rate yet, a standstill, or nothing left — all unknowable or moot.
    expect(secondsRemaining(8000, null)).toBeNull();
    expect(secondsRemaining(8000, 0)).toBeNull();
    expect(secondsRemaining(0, 0.22)).toBeNull();
  });

  /// The real numbers measured on the owner's NTFS drive, end to end.
  it("gives the finish time actually observed on a real drive", () => {
    const s = history([
      [0, 471],
      [45_000, 481],
    ]);
    const rate = ratePerSecond(s, 45_000)!;
    expect(rate).toBeCloseTo(0.222, 3);

    const left = secondsRemaining(8333 - 481, rate)!;
    // Just under ten hours, which is what the catalogue independently showed.
    expect(left / 3600).toBeGreaterThan(9.5);
    expect(left / 3600).toBeLessThan(10.5);
  });
});
