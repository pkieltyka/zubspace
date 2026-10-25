import assert from "node:assert/strict";
import test from "node:test";

import {
  LONGTASK_FLOOR_MS,
  STARVATION_FLOOR_MS,
  assertWindowFrameGaps,
  attributeFrameGaps,
  frameGapDelta,
  longTaskWitnessInstallScript,
  longTaskWitnessReadScript,
  longTasksInWindow,
  starvationInWindow,
} from "./long_task_witness.mjs";

// The install and read scripts run inside the browser, so a typo here is a lane that silently
// measures nothing. These run the real script text against a fake page instead, which is also the
// only way to pin the accounting rules (edge intervals, double install, capped lists) without
// waiting for an organic stall.
function fakePage({ floorMs = LONGTASK_FLOOR_MS, startAt = 1_000 } = {}) {
  const page = {
    time: startAt,
    rafQueue: [],
    timers: [],
    observers: [],
    performance: null,
  };
  page.performance = { timeOrigin: 1_700_000_000_000, now: () => page.time };
  page.requestAnimationFrame = (callback) => {
    page.rafQueue.push(callback);
    return page.rafQueue.length;
  };
  page.setTimeout = (callback, delayMs) => {
    page.timers.push({ callback, dueAt: page.time + (Number(delayMs) || 0) });
    return page.timers.length;
  };
  page.PerformanceObserver = class FakePerformanceObserver {
    static supportedEntryTypes = ["longtask"];
    constructor(callback) {
      this.callback = callback;
      page.observers.push(this);
    }
    observe() {}
  };
  page.window = {};
  page.run = (script) => new Function(
    "window", "performance", "requestAnimationFrame", "PerformanceObserver", "setTimeout",
    `return ${script}`,
  )(page.window, page.performance, page.requestAnimationFrame, page.PerformanceObserver,
    page.setTimeout);
  page.install = (options) => page.run(longTaskWitnessInstallScript(
    typeof options === "number" ? { gapFloorMs: options } : (options ?? { gapFloorMs: floorMs }),
  ));
  page.read = () => JSON.parse(page.run(longTaskWitnessReadScript()));
  page.tick = (deltaMs) => {
    page.time += deltaMs;
    const pending = page.rafQueue.splice(0, page.rafQueue.length);
    for (const callback of pending) callback(page.time);
  };
  // One drain runs one generation of the probe's repost loop, so a caller controls exactly how long
  // the main thread stayed away between a schedule and its handling. A repost lands in the next
  // generation, which is what keeps a busy page's timer queue finite.
  page.drainTasks = (deltaMs = 0) => {
    page.time += deltaMs;
    const due = page.timers.splice(0, page.timers.length);
    for (const timer of due) if (timer.dueAt <= page.time) timer.callback();
  };
  page.reportLongTask = ({ start, duration }) => {
    for (const observer of page.observers) {
      observer.callback({ getEntries: () => [{ startTime: start, duration }] });
    }
  };
  return page;
}

test("the frame witness counts a stall from the install instant, not the first callback", () => {
  const page = fakePage({ startAt: 5_000 });
  assert.equal(page.install(), "observing");
  // Nothing has been drawn yet and the first callback is already 240ms after the install. Seeding
  // `previous` with the install instant is what makes this a gap rather than an exempt edge.
  page.tick(240);
  const read = page.read();
  assert.equal(read.frames, 1);
  assert.equal(read.gapCount, 1);
  assert.equal(read.maximumGapMs, 240);
  assert.equal(read.gaps.length, 1);
  assert.equal(read.gapFloorMs, LONGTASK_FLOOR_MS);
  assert.equal(read.longTaskSupport, true);
});

test("the frame witness ignores intervals under the floor and stops at the cap", () => {
  const page = fakePage();
  page.install(100);
  page.tick(20);
  page.tick(99);
  assert.equal(page.read().gapCount, 0, "99ms under a 100ms floor is not a gap");
  page.tick(150);
  assert.equal(page.read().gapCount, 1);
  for (let index = 0; index < 80; index += 1) page.tick(120);
  const read = page.read();
  assert.ok(read.gapCount > 64, `the counter keeps counting past the list cap, got ${read.gapCount}`);
  assert.equal(read.gaps.length, 64, "the retained gap list stays bounded");
});

test("the witness survives a second install and keeps the running counters", () => {
  const page = fakePage();
  assert.equal(page.install(), "observing");
  page.tick(300);
  assert.equal(page.install(), "present");
  page.tick(120);
  const read = page.read();
  assert.equal(read.frames, 2, "a re-install must not restart the frame counter");
  assert.equal(read.gapCount, 2);
});

test("long tasks are attributed to the window that contains their start", () => {
  const page = fakePage();
  page.install();
  page.tick(16);
  const before = page.read();
  page.reportLongTask({ start: page.time + 40, duration: 412 });
  page.reportLongTask({ start: page.time + 900, duration: 90 });
  page.reportLongTask({ start: 12, duration: 900 });
  page.tick(2_000);
  const after = page.read();
  const window = frameGapDelta(before, after);
  assert.equal(window.installed, true);
  assert.deepEqual(window.longTasks.map((task) => task.duration), [412, 90],
    "a long task recorded before the window opened must not be charged to it");
  assert.deepEqual(longTasksInWindow([{ start: 10, duration: 5 }], { fromMs: 0, toMs: 20 }), [
    { start: 10, duration: 5 },
  ]);
});

test("a capped gap list is reported as a lower bound instead of a smaller maximum", () => {
  const window = frameGapDelta(
    { now: 1_000, frames: 10, gapCount: 70, gaps: [{ at: 1_200, gapMs: 210 }], maximumGapMs: 1_130, longTasks: [] },
    { now: 2_000, frames: 610, gapCount: 140, gaps: [{ at: 1_200, gapMs: 210 }], maximumGapMs: 1_130, longTaskCount: 0, longTasks: [] },
  );
  assert.equal(window.windowMs, 1_000);
  assert.equal(window.frames, 600);
  assert.equal(window.gapCount, 70);
  assert.equal(window.truncated, true);
  assert.equal(window.maximumGapMs, 1_130);
  assert.equal(window.maximumGapMsIsLowerBound, true);
});

test("a clean window passes and a stalled one names the floor, the worst gap and the long task", () => {
  const clean = frameGapDelta(
    { now: 0, frames: 60, gapCount: 0, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [], maximumGapMs: 33, longTasks: [] },
    { now: 1_000, frames: 120, gapCount: 0, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [], maximumGapMs: 33, longTaskCount: 0, longTasks: [] },
  );
  assert.equal(assertWindowFrameGaps(clean, "arena switch").gapCount, 0);

  const stalled = frameGapDelta(
    { now: 0, frames: 60, gapCount: 0, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [], maximumGapMs: 33, longTasks: [] },
    {
      now: 1_000, frames: 110, gapCount: 1, gapFloorMs: LONGTASK_FLOOR_MS, maximumGapMs: 137, longTaskCount: 1,
      gaps: [{ at: 500, gapMs: 137 }], longTasks: [{ start: 480, duration: 129 }],
    },
  );
  assert.throws(
    () => assertWindowFrameGaps(stalled, "wormhole relocation"),
    (error) => error.message.includes("wormhole relocation") &&
      error.message.includes("1 frame gap") &&
      error.message.includes("80ms floor") &&
      error.message.includes('"gapMs":137') &&
      error.message.includes('"duration":129'),
  );
  assert.throws(() => assertWindowFrameGaps(null, "arena switch"), /has no frame witness installed/);
});

test("a task-only install leaves the page's own animation callbacks alone", () => {
  const page = fakePage();
  page.time = 1_000;
  assert.equal(page.run(longTaskWitnessInstallScript({ frameSampler: false })), "observing");
  assert.equal(page.rafQueue.length, 0, "no animation callback may be queued by a task-only witness");
  page.reportLongTask({ start: 1_020, duration: 96 });
  const read = page.read();
  assert.equal(read.frames, 0);
  assert.equal(read.gapCount, 0);
  assert.deepEqual(read.longTasks, [{ start: 1_020, duration: 96 }]);
});

test("a clean window still carries page-life gap timestamps beside its own bounds", () => {
  // The window-only view once reported `gapCount: 0` while the same page had a 578 ms hitch. The
  // delta has to carry the page-life list with absolute timestamps, or a reader cannot tell a
  // window that contains nothing from a window placed in the wrong place.
  const clean = frameGapDelta(
    { now: 1_000, frames: 60, gapCount: 1, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [{ at: 400, gapMs: 578 }], maximumGapMs: 578, longTasks: [] },
    { now: 2_000, frames: 660, gapCount: 1, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [{ at: 400, gapMs: 578 }], maximumGapMs: 578, longTaskCount: 0, longTasks: [] },
  );
  assert.equal(clean.gapCount, 0);
  assert.deepEqual(clean.gaps, []);
  assert.equal(clean.pageWideMaximumGapMs, 578);
  assert.deepEqual(clean.pageWideGaps, [{ at: 400, gapMs: 578 }]);
  assert.equal(clean.pageWideGapsTruncated, false);

  const capped = frameGapDelta(clean, {
    now: 3_000, frames: 1_260, gapCount: 40, gapFloorMs: LONGTASK_FLOOR_MS,
    gaps: [{ at: 400, gapMs: 578 }, { at: 2_400, gapMs: 91 }], maximumGapMs: 578, longTaskCount: 0, longTasks: [],
  });
  assert.equal(capped.pageWideGaps.length, 2);
  assert.equal(capped.pageWideGapsTruncated, true, "40 gaps recorded behind a 2-entry list");
});

test("the witness keeps every frame interval, so a sub-floor merge cost is visible", () => {
  // A zone merge measured at 60ms never trips an 80ms floor. Without the interval ring the window
  // would report "0 gaps" and a reader would take that for "the frames were fine".
  const page = fakePage();
  page.install();
  page.tick(16);
  const before = page.read();
  for (const interval of [17, 16, 60, 18, 45, 16]) page.tick(interval);
  const window = frameGapDelta(before, page.read());
  assert.equal(window.gapCount, 0, "none of these intervals trips the 80ms floor");
  assert.equal(window.frameCost.samples, 6);
  assert.equal(window.frameCost.maximumMs, 60);
  assert.equal(window.frameCost.atLeast34Ms, 2);
  assert.equal(window.frameCost.atLeast50Ms, 1);
  assert.equal(window.frameCost.atLeast80Ms, 0);
  assert.equal(window.frameCost.truncated, false);
  assert.ok(window.frameCost.totalMs >= 170, `sums the window, got ${window.frameCost.totalMs}`);
});

test("an interval ring that evicts says so instead of reporting a smaller window cost", () => {
  const page = fakePage();
  page.run(longTaskWitnessInstallScript({ intervalRingLimit: 3 }));
  page.tick(16);
  const before = page.read();
  assert.equal(before.intervalEvictions, 0);
  for (let index = 0; index < 9; index += 1) page.tick(20);
  const after = page.read();
  assert.equal(after.intervals.length, 3, "the retained ring stays bounded");
  assert.ok(after.intervalEvictions > 0, `evictions must be counted, got ${after.intervalEvictions}`);
  const window = frameGapDelta(before, after);
  assert.equal(window.frameCost.truncated, true);
});

test("a window with no interval samples reports zero samples rather than a zero maximum", () => {
  const window = frameGapDelta(
    { now: 1_000, frames: 0, gapCount: 0, gaps: [], intervals: [], intervalEvictions: 0, maximumGapMs: 0, longTasks: [] },
    { now: 1_500, frames: 0, gapCount: 0, gaps: [], intervals: [], intervalEvictions: 0, maximumGapMs: 0, longTaskCount: 0, longTasks: [] },
  );
  assert.equal(window.frameCost.samples, 0);
  assert.equal(window.frameCost.maximumMs, undefined);
});

test("the starvation probe stays off unless a lane asks for its task loop", () => {
  const page = fakePage();
  page.install();
  assert.equal(page.timers.length, 0, "a lane that did not ask must not gain a live task loop");
  page.tick(16);
  const read = page.read();
  assert.equal(read.starvationTicks, 0);
  assert.equal(read.starvationSupport, false);
  assert.deepEqual(read.starvation, []);
});

test("the probe keeps one timer pending, so it cannot starve the page it measures", () => {
  // The first version reposted through a `MessageChannel`, which the event loop dispatches without
  // respect for the frame budget: live Firefox dropped to 85ms frames with 181 gaps in one 20s
  // window, so the instrument had become the stall it was installed to name. One pending timer per
  // generation is what keeps an observer out of its own measurement.
  const page = fakePage();
  page.install({ starvationProbe: true });
  assert.equal(page.timers.length, 1, "the probe must prime exactly one scheduled tick");
  for (const delay of [4, 6, 5, 7, 4]) {
    page.drainTasks(delay);
    assert.equal(page.timers.length, 1, "a probe generation may schedule only its successor");
    assert.equal(page.rafQueue.length, 1, "the probe must not consume the page's frame callback");
  }
});

test("the probe measures how long the main thread stayed away from a scheduled task", () => {
  const page = fakePage();
  page.install({ starvationProbe: true });
  assert.equal(page.timers.length, 1, "the probe must prime its own repost loop");
  page.drainTasks(4);
  page.drainTasks(640);
  page.drainTasks(6);
  const read = page.read();
  assert.equal(read.starvationSupport, true);
  assert.equal(read.starvationTicks, 3);
  assert.equal(read.starvationMaximumMs, 640);
  assert.deepEqual(read.starvation, [{ at: 1_000 + 4 + 640, delayMs: 640, framesAdvanced: 0,
    hidden: false }],
    "only a delay worth a stall claim consumes a slot, stamped at the instant it was handled, and it "
    + "carries whether any animation callback ran while the timer waited");
});

test("a frame gap is attributed to a busy main thread only when the probe waited too", () => {
  // Both readings below hold the same 640ms frame gap; only the probe separates a thread that was
  // executing something else from a browser that simply never delivered a frame.
  const busy = attributeFrameGaps([{ at: 700, gapMs: 640 }], {
    starvation: [{ at: 690, delayMs: 610 }],
    starvationSupport: true,
    windowTicks: 3,
    windowMs: 5_000,
  });
  assert.equal(busy[0].attribution, "main-thread-busy");
  assert.equal(busy[0].starvation.maximumMs, 610);

  const notDelivered = attributeFrameGaps([{ at: 700, gapMs: 640 }], {
    starvation: [],
    starvationSupport: true,
    windowTicks: 5_000,
    windowMs: 5_000,
  });
  assert.equal(notDelivered[0].attribution, "frame-not-delivered");

  assert.equal(attributeFrameGaps([{ at: 700, gapMs: 640 }], {
    starvation: [], starvationSupport: true, windowTicks: 0, windowMs: 5_000,
  })[0].attribution, "probe-silent");
  assert.equal(attributeFrameGaps([{ at: 700, gapMs: 640 }], {
    starvation: [{ at: 690, delayMs: 610 }], starvationSupport: false, windowTicks: 9_000, windowMs: 5_000,
  })[0].attribution, "probe-unsupported");
  // A window holding only a handful of probe tasks spent its time inside one of them, however small
  // each recorded delay looked on its own.
  assert.equal(attributeFrameGaps([{ at: 700, gapMs: 640 }], {
    starvation: [], starvationSupport: true, windowTicks: 5, windowMs: 5_000,
  })[0].attribution, "main-thread-busy");
});

test("a starvation delay covers the span it waited across, not just the instant it was handled", () => {
  const entries = [{ at: 500, delayMs: 120 }, { at: 900, delayMs: 40 }];
  // The gap runs 360..420; the first entry waited 380..500, so it intersects even though it was
  // handled after the gap ended.
  assert.deepEqual(starvationInWindow(entries, { fromMs: 360, toMs: 420 }).entries, [entries[0]]);
  assert.equal(starvationInWindow(entries, { fromMs: 600, toMs: 700 }).samples, 0);
  assert.ok(STARVATION_FLOOR_MS > 0);
});

test("the window delta carries the probe verdict next to the gaps it explains", () => {
  const before = {
    now: 1_000, startedAt: 0, frames: 60, gapCount: 0, gapFloorMs: LONGTASK_FLOOR_MS, gaps: [],
    intervals: [], intervalEvictions: 0, maximumGapMs: 20, longTasks: [],
    starvationTicks: 900, starvationSupport: true, starvationFloorMs: STARVATION_FLOOR_MS,
    starvationMaximumMs: 12, starvation: [], starvationEvictions: 0,
  };
  const after = {
    ...before, now: 6_000, frames: 360, gapCount: 1, maximumGapMs: 635,
    gaps: [{ at: 3_579, gapMs: 635 }],
    starvationTicks: 4_900, starvationMaximumMs: 601,
    starvation: [{ at: 3_560, delayMs: 601 }],
  };
  const window = frameGapDelta(before, after);
  assert.equal(window.starvation.support, true);
  assert.equal(window.starvation.ticks, 4_000);
  assert.equal(window.starvation.windowMaximumMs, 601);
  assert.deepEqual(window.gapsAttributed.map((gap) => gap.attribution), ["main-thread-busy"]);
  assert.deepEqual(window.pageWideGapsAttributed.map((gap) => gap.attribution), ["main-thread-busy"]);
});

test("a caller that needs more than the installed witness can attach to it", () => {
  const page = fakePage({ startAt: 2_000 });
  // This is the shape every Chrome client page gets from its bootstrap: long tasks only, no
  // animation callback. A later lane that needs frame cadence must be able to attach, not be told
  // the witness is "present" and silently measure nothing.
  assert.equal(page.install({ frameSampler: false, starvationProbe: false }), "observing");
  page.tick(60);
  assert.equal(page.read().frames, 0, "the bootstrap witness runs no animation callback");
  assert.equal(page.install({ frameSampler: true, starvationProbe: true }),
    "upgraded:frameSampler,starvationProbe");
  page.tick(300);
  page.drainTasks(0);
  const read = page.read();
  assert.equal(read.frames, 1, "the sampler counts from the moment it was attached");
  assert.equal(read.gapCount, 1, "a stall after attachment is a gap, not an exempt edge");
  assert.equal(read.maximumGapMs, 300);
  assert.equal(read.frameSamplerRunning, true);
  assert.equal(read.starvationTicks >= 1, true, "the probe runs from the moment it was attached");
});

test("a repeated install cannot run two samplers on one witness", () => {
  const page = fakePage();
  page.install({ frameSampler: true });
  assert.equal(page.install({ frameSampler: true, starvationProbe: true }), "upgraded:starvationProbe");
  page.tick(16);
  const read = page.read();
  assert.equal(read.frames, 1, "one animation callback advances the counter by exactly one");
  assert.equal(read.intervals.length, 1, "and records exactly one frame interval");
  assert.equal(read.intervals[0].intervalMs, 16);
  assert.equal(page.install({ frameSampler: true, starvationProbe: true }), "present");
});

test("an install that cannot apply a requested floor says so instead of reporting success", () => {
  // Fresh install: the requested floor is what the witness runs with.
  const page = fakePage({ floorMs: 40 });
  assert.equal(page.install({ gapFloorMs: 40 }), "observing");
  assert.equal(page.window.__zubspaceFrameWitness.gapFloorMs, 40);

  // Second install while it is already running: the running witness keeps its floor, because counts
  // taken on either side of a retune would not be comparable. It must not be ignored silently - a
  // caller that asked for 40ms and got an 80ms ring would otherwise read an empty window and report
  // "no stalls happened", which is an instrument choice turning into a false negative.
  const status = page.install({ gapFloorMs: 120, starvationFloorMs: 5, frameSampler: false });
  assert.equal(page.window.__zubspaceFrameWitness.gapFloorMs, 40, "not retuned mid-run");
  assert.match(status, /gapFloorMs=40 \(requested 120 not applied\)/,
    "the unapplied gap floor is named in the returned status");
  assert.match(status, /starvationFloorMs=\d+ \(requested 5 not applied\)/,
    "and so is the unapplied starvation floor");
});

test("a timer that waited while frames kept running is scheduling, not occupation", async () => {
  const { starvationInWindow, starvationInsideGap } = await import("./long_task_witness.mjs");
  const window = { fromMs: 1000, toMs: 6000 };
  // The browser kept drawing and simply preferred rendering to a zero-delay timer.
  const scheduling = starvationInWindow([
    { at: 2000, delayMs: 88, framesAdvanced: 3 },
    { at: 3000, delayMs: 61, framesAdvanced: 2 },
  ], window);
  assert.equal(scheduling.maximumMs, 0, "no occupation claim may come out of scheduling waits");
  assert.equal(scheduling.timerWaitMaximumMs, 88);
  assert.equal(scheduling.busySamples, 0);
  assert.equal(scheduling.timerWaitSamples, 2);

  // The thread really was away: no frame ran during the interval.
  const occupied = starvationInWindow([
    { at: 2000, delayMs: 88, framesAdvanced: 3 },
    { at: 4500, delayMs: 140, framesAdvanced: 0 },
  ], window);
  assert.equal(occupied.maximumMs, 140);
  assert.equal(occupied.busySamples, 1);
  assert.equal(occupied.timerWaitSamples, 1);

  // The rate, not the count, is what distinguishes the two. Two frames in 727ms is a page drawing at
  // about 3 fps - a stall - while three frames in 88ms is the page's normal cadence. A count rule
  // labelled the first case "rendering normally", and which verdict it got depended on whether the
  // release frame happened to land inside the wait.
  const threeFps = starvationInWindow([{ at: 8657, delayMs: 727, framesAdvanced: 2 }],
    { fromMs: 7900, toMs: 9000 });
  assert.equal(threeFps.busySamples, 1, "two frames in 727ms is not a page that kept drawing");
  assert.equal(threeFps.maximumMs, 727);
  const normalCadence = starvationInWindow([
    { at: 2088, delayMs: 88, framesAdvanced: 3 },
    { at: 2940, delayMs: 40, framesAdvanced: 1 },
  ], window);
  assert.equal(normalCadence.busySamples, 0, "one frame per 40ms is a page drawing at its real rate");
  assert.equal(normalCadence.timerWaitMaximumMs, 88);

  // A wait that releases exactly into the next frame contains that frame. One boundary frame does not
  // mean the page was drawing: this is the shape of a 626ms block that ends when the callback runs.
  const boundaryOnly = starvationInWindow(
    [{ at: 4500, delayMs: 626, framesAdvanced: 1 }], window);
  assert.equal(boundaryOnly.busySamples, 1, "one boundary frame does not clear a block");
  assert.equal(boundaryOnly.maximumMs, 626);
  assert.equal(boundaryOnly.timerWaitSamples, 0);

  // A block longer than the probe's repost interval is one interval that straddles the gap. Containment
  // alone discards it, which is how a 713ms freeze read back as "no samples, frames simply were not
  // delivered" - the opposite verdict. A covering wait counts, contributing only its in-gap part.
  const covered = starvationInsideGap(
    [{ at: 8714, delayMs: 900, framesAdvanced: 0 }], { fromMs: 8001, toMs: 8714 });
  assert.equal(covered.coveringSamples, 1);
  assert.equal(covered.busySamples, 1, "a wait covering the whole gap speaks for the whole gap");
  assert.equal(covered.maximumMs, 713, "and contributes only the part inside it, not its own length");
  const coveredButRendering = starvationInsideGap(
    [{ at: 8714, delayMs: 900, framesAdvanced: 24 }], { fromMs: 8001, toMs: 8714 });
  assert.equal(coveredButRendering.busySamples, 0, "a covering wait that drew 24 frames was not a block");
  assert.equal(coveredButRendering.timerWaitMaximumMs, 713);

  // An entry with no observation is neither: it is disclosed, not promoted.
  const legacy = starvationInWindow([{ at: 2000, delayMs: 90 }], window);
  assert.equal(legacy.unknownSamples, 1);
  assert.equal(legacy.maximumMs, 90, "legacy entries keep the reading they were archived with");
});

test("a gap whose only long waits were scheduling waits is not called a busy thread", async () => {
  const { attributeFrameGaps } = await import("./long_task_witness.mjs");
  // The wait has to lie inside the gap to describe it: 1930-1990 is contained by 1910-2000.
  const attributed = attributeFrameGaps([{ at: 2000, gapMs: 90 }], {
    starvation: [{ at: 1990, delayMs: 60, framesAdvanced: 4 }],
    starvationSupport: true, windowTicks: 300, windowMs: 5000, floorMs: 50,
  });
  assert.equal(attributed[0].attribution, "timer-starved-while-rendering");
  assert.equal(attributed[0].starvation.busySamples, 0);
  assert.equal(attributed[0].starvation.timerWaitMaximumMs, 60);
});

test("an entry that only straddles a gap edge cannot speak for the whole gap", async () => {
  const { starvationInsideGap } = await import("./long_task_witness.mjs");
  const gap = { fromMs: 1000, toMs: 1700 };
  // One long wait that began long before the gap and ended inside it, described by frames that ran
  // outside the gap. Containment is what stops it being read as the gap's own occupation.
  const straddling = starvationInsideGap(
    [{ at: 1100, delayMs: 900, framesAdvanced: 30 }], gap);
  assert.equal(straddling.samples, 0, "an interval that starts before the gap is not inside it");
  assert.equal(straddling.timerWaitMaximumMs, 0);
  // An interval fully inside the gap does describe it.
  const contained = starvationInsideGap(
    [{ at: 1600, delayMs: 550, framesAdvanced: 0 }], gap);
  assert.equal(contained.busySamples, 1);
  assert.equal(contained.maximumMs, 550);
});

test("a frame gap the thread was free through is not called a busy thread", async () => {
  const { attributeFrameGaps } = await import("./long_task_witness.mjs");
  // Timers kept running through the whole no-frame interval, so nothing occupied the thread.
  const free = attributeFrameGaps([{ at: 1700, gapMs: 700 }], {
    starvation: [{ at: 1300, delayMs: 8, framesAdvanced: 0 }],
    starvationSupport: true, windowTicks: 180, windowMs: 700, floorMs: 50,
  });
  assert.equal(free[0].attribution, "frame-not-delivered",
    "a thread that kept dispatching short timer ticks across a frame gap is free, not busy");
});

test("the generated page scripts parse, which catches the comment characters that break them", async () => {
  // A backtick inside a comment in a generated page script terminates the template literal that builds
  // it, and the module still loads fine - the damage only appears when a browser is handed the string.
  // This has happened three times, so the strings are parsed rather than assumed.
  const mod = await import("./long_task_witness.mjs");
  const install = mod.longTaskWitnessInstallScript({});
  const read = mod.longTaskWitnessReadScript({});
  const ringSpan = /starvationRingFromMs|starvationRingToMs/;
  assert.ok(ringSpan.test(read), "the read reports the ring's time span, not only its contents");
  for (const [name, script] of [["install", install], ["read", read]]) {
    // Parsed as a function body, not executed: these are immediately-invoked page expressions, and
    // running them here would evaluate against a page that does not exist.
    assert.doesNotThrow(() => new Function(script), `${name} script must parse as a page script body`);
    assert.match(script, /^\(\(\) => \{/, `${name} script stays a self-contained page expression`);
  }
});

test("the window summary says how far the retained probe ring actually reaches", async () => {
  const { frameGapDelta } = await import("./long_task_witness.mjs");
  const before = { now: 1000, frames: 10, gapCount: 0, intervalEvictions: 0, starvationTicks: 5,
    starvationEvictions: 0, starvation: [] };
  const after = { now: 6000, frames: 120, gapCount: 1, intervalEvictions: 0, starvationTicks: 900,
    starvationEvictions: 0, starvation: [{ at: 4000, delayMs: 12, framesAdvanced: 1 }],
    starvationRingFromMs: 200, starvationRingToMs: 4000, starvationSupport: true };
  const delta = frameGapDelta(before, after);
  assert.equal(delta.starvationRingFromMs, 200);
  assert.equal(delta.starvationRingToMs, 4000);
  const missing = frameGapDelta({ ...before }, { ...after, starvationRingFromMs: null,
    starvationRingToMs: null });
  assert.equal(missing.starvationRingToMs, null, "an unknown span is reported as unknown, not as full coverage");
});
