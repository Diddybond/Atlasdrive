/// Working out how fast a scan is going, and when it will finish.
///
/// Pulled out of the component so it can be tested directly. Verifying it
/// through the interface meant watching a browser tab, and a *hidden* tab has
/// its timers throttled to roughly once a minute — which starves the sample
/// window and makes a perfectly good calculation look broken. Arithmetic this
/// load-bearing should not be checked by squinting at a screenshot.

export interface Sample {
  at: number;
  done: number;
}

/// The shortest span worth dividing by.
///
/// Below this the interval is short enough that one photograph landing early or
/// late swings the figure wildly, and a dashboard that reports 400/min then
/// 3/min a second later is worse than one that admits it does not know yet.
export const MIN_SPAN_MS = 20_000;

/// How far back to look. Long enough to be steady — one photograph takes a
/// second or twenty depending on how many faces are in it — and short enough to
/// reflect the drive being read now rather than an average over the whole night.
export const WINDOW_MS = 3 * 60 * 1000;

/// Add a reading, keeping only what is inside the window.
///
/// A reading that does not move the count is dropped, so a stalled scan widens
/// the span between the samples it does have and its rate falls towards zero,
/// rather than holding the last healthy figure and looking fine.
export function record(samples: Sample[], now: number, done: number): Sample[] {
  const next = samples.length === 0 || samples[samples.length - 1].done !== done
    ? [...samples, { at: now, done }]
    : [...samples];
  while (next.length > 2 && now - next[0].at > WINDOW_MS) next.shift();
  return next;
}

/// Units per second, or `null` when there is not yet enough evidence to say.
export function ratePerSecond(samples: Sample[], now = Date.now()): number | null {
  if (samples.length < 2) return null;
  const first = samples[0];
  const last = samples[samples.length - 1];
  // Measured to *now*, not to the last reading. If nothing has arrived for two
  // minutes the rate must reflect that, and measuring only between readings
  // would report the old healthy speed forever.
  const span = (Math.max(now, last.at) - first.at) / 1000;
  if (span * 1000 < MIN_SPAN_MS) return null;
  const moved = last.done - first.done;
  return moved > 0 ? moved / span : 0;
}

/// Seconds until `remaining` items are done, or `null` when unknowable.
export function secondsRemaining(remaining: number, rate: number | null): number | null {
  if (rate === null || rate <= 0 || remaining <= 0) return null;
  return remaining / rate;
}
