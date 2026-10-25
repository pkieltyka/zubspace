// One instrument for "the page's main thread did not come back on time".
//
// Three lanes needed this and each grew its own copy: a bare long-task observer in
// `combat_replay.mjs` and `frame_cadence_probe.mjs`, and a frame-gap sampler fused into the
// ordinary-URL capture in `ordinary_url.mjs`. That copy in `ordinary_url.mjs` stays where it is on
// purpose: its rAF loop is also the thing under test there (it samples the real request gates and
// asserts that the ordinary URL publishes nothing), so lifting it would move the measurement into
// the instrument. Everything else reads this module.
//
// Two instruments, because they answer different questions. A `longtask` entry attributes a stall
// to one synchronous task on the main thread, and `buffered: true` recovers entries recorded before
// the observer was installed, so a boot phase stays visible. A frame gap is what the player sees:
// the wall-clock distance between two `requestAnimationFrame` callbacks, which catches a stall split
// across tasks (or a throttled tab) that no single long task owns. The 80ms floor is the one the
// ordinary-URL gate already uses, so a stall cannot pass one lane and fail the other.
//
// A third instrument exists because a `longtask` entry is a Chrome-only privilege. Firefox reports
// neither `longtask` nor `long-animation-frame` in `PerformanceObserver.supportedEntryTypes`, so on
// that engine a frame gap has nothing to name: the retained Firefox arms of the wormhole gate record
// `longTaskSupport: false` beside a 643 ms page freeze and no task attribution at all. The starvation
// probe is the engine-independent answer. A timer task runs when the main thread gets to it, so the
// distance between scheduling and handling it measures "the main thread was executing something
// else", whatever that something was - GC, JIT, browser-internal work, or this page's own non-frame
// task. Read next to a frame gap it separates a thread that was busy from a callback that was simply
// never scheduled, which is the question the `longtask` entry answers for free in Chrome.
//
// It reposts through a timer, not a `MessageChannel`. Measured the first time it ran, a self-posting
// channel starved the Firefox page to 85 ms frames (11.7 fps, 181 gaps in a 20 s window) because a
// channel task is dispatched without respect for the frame budget - the instrument became the
// stall. A zero-delay timer is clamped to about 4ms once nesting is deep, which yields to rendering
// and still samples the thread at ~250Hz: far finer than the 34ms floor it reports against.

// Both instruments accumulate for the page's whole life, so a window - an arena switch, a wormhole
// relocation - is measured by reading before and after and diffing. The gap list is capped, and the
// helpers say so instead of quietly reporting the smaller maximum.

export const LONGTASK_FLOOR_MS = 80;

// Below two frames of starvation the probe cannot tell anything apart; above it, every delay is
// worth a slot because a stall can hide in the gap between two probe ticks. It sits above the timer
// nesting clamp on purpose: a healthy page reports 4-17ms here.
export const STARVATION_FLOOR_MS = 34;

// Installs the long-task observer, and by default the frame-gap sampler with it. A lane that only
// wants task attribution (the cadence probe, which already runs its own rAF counter) passes
// `frameSampler: false` so the instrument does not add a second animation callback to the page whose
// cadence it is measuring. Safe to evaluate twice either way: the second call reports "present" and
// leaves the running witness alone, because a lane that reinstalled mid-window would reset the
// counters the window diff depends on.
export function longTaskWitnessInstallScript({
  gapFloorMs = LONGTASK_FLOOR_MS,
  frameSampler = true,
  intervalRingLimit = 1200,
  starvationProbe = false,
  starvationFloorMs = STARVATION_FLOOR_MS,
  starvationRingLimit = 256,
} = {}) {
  return `(() => {
  // Both instruments are separate producers that write into one witness, and either can be attached
  // after the fact. A page bootstrap may already have installed the task-only witness - that is what
  // every Chrome client gets - and a caller that later needs frame cadence or main-thread
  // attribution must be able to attach it, not be told the witness is "present" and quietly get
  // nothing. Each start is idempotent, so a double install cannot run two samplers and double-count
  // a frame.
  const startFrameSampler = (witness, seedMs) => {
    if (witness.frameSamplerRunning || typeof requestAnimationFrame !== "function") { return false; }
    witness.frameSamplerRunning = true;
    witness.frameSampler = true;
    witness.frameSamplerStartedAt = Math.round(performance.now());
    // The opening interval is visible time too: seeding the previous-sample stamp with the start
    // instant means a stall before the first callback is a gap, not an exempt edge.
    let previous = seedMs;
    const step = () => {
      const at = performance.now();
      witness.frames += 1;
      const gapMs = at - previous;
      previous = at;
      if (gapMs > witness.maximumGapMs) witness.maximumGapMs = gapMs;
      if (gapMs >= witness.gapFloorMs && witness.gaps.length < 64) {
        witness.gaps.push({ at: Math.round(at), gapMs: Math.round(gapMs) });
      }
      if (gapMs >= witness.gapFloorMs) witness.gapCount += 1;
      // Every interval, not just the ones past the floor. A zone merge measured at 60ms never trips
      // an 80ms floor and still costs a third of a second of frame time across fourteen archives, so
      // a window needs the whole distribution. Preallocated and written in place: this runs inside
      // the page's own animation callback, so shifting a live array each frame would add work to the
      // very cadence the instrument reports.
      witness.intervalRing[witness.intervalWrite % witness.intervalLimit] =
        { at: Math.round(at), intervalMs: Math.round(gapMs * 10) / 10 };
      witness.intervalWrite += 1;
      if (witness.intervalWrite > witness.intervalLimit) {
        witness.intervalEvictions = witness.intervalWrite - witness.intervalLimit;
      }
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
    return true;
  };
  const startStarvationProbe = (witness) => {
    if (witness.starvationProbeRunning || typeof setTimeout !== "function") { return false; }
    witness.starvationProbeRunning = true;
    witness.starvationProbeStartedAt = Math.round(performance.now());
    // The probe reposts from its own handler, one pending timer at a time: a healthy page spends
    // microseconds per tick and yields to rendering between every one of them. It must never repost
    // through a MessageChannel - port messages are dispatched without respect for the frame budget,
    // which starves rendering and makes the instrument the stall it was installed to name.
    let postedAt = performance.now();
    let framesAtPost = witness.frames;
    const probeTick = () => {
      const handledAt = performance.now();
      const delayMs = handledAt - postedAt;
      // Whether the frame sampler ran during this interval is what separates the two things a long
      // timer wait can mean. If no frame ran, the thread was occupied or rendering was suspended, and
      // the interval is real stall evidence. If frames did run, the browser simply preferred rendering
      // to a zero-delay timer: that is scheduling, not occupation, and counting it as a stall is how a
      // probe reports 88ms of a busy main thread inside a window whose worst frame gap was 32.9ms.
      const framesAdvanced = witness.frames - framesAtPost;
      framesAtPost = witness.frames;
      witness.starvationSupport = true;
      witness.starvationTicks += 1;
      // Same threshold the Node-side classifier uses: a wait that releases into a single boundary
      // frame is a block, so the page-wide maximum has to count it. Reporting a 12ms page maximum next
      // to a 688ms inside-gap maximum - which is what a framesAdvanced === 0 rule produced - would read as the
      // gap being misclassified rather than the maximum being under-counted.
      // The same rate rule as the Node-side classifier, compiled in: a wait is occupation unless the
      // page drew at least one frame per ${SCHEDULING_MAX_MS_PER_FRAME}ms of the wait. Renaming the old
      // frame-count constant without touching this interpolation would have thrown while building the
      // install script, which is how an instrument silently stops existing.
      const drawnEnough = framesAdvanced > 0 && framesAdvanced * ${SCHEDULING_MAX_MS_PER_FRAME} >= delayMs;
      if (!drawnEnough && delayMs > witness.starvationMaximumMs) {
        witness.starvationMaximumMs = delayMs;
      }
      if (delayMs >= witness.starvationFloorMs) {
        witness.starvationRing[witness.starvationWrite % witness.starvationLimit] =
          { at: Math.round(handledAt), delayMs: Math.round(delayMs * 10) / 10,
            framesAdvanced,
            hidden: typeof document !== "undefined" && document.visibilityState === "hidden" };
        witness.starvationWrite += 1;
        if (witness.starvationWrite > witness.starvationLimit) {
          witness.starvationEvictions = witness.starvationWrite - witness.starvationLimit;
        }
      }
      postedAt = handledAt;
      setTimeout(probeTick, 0);
    };
    setTimeout(probeTick, 0);
    return true;
  };
  const wantFrameSampler = ${frameSampler ? "true" : "false"};
  const wantStarvationProbe = ${starvationProbe ? "true" : "false"};
  const existing = window.__zubspaceFrameWitness;
  if (existing) {
    const attached = [];
    if (wantFrameSampler && startFrameSampler(existing, performance.now())) attached.push("frameSampler");
    if (wantStarvationProbe && startStarvationProbe(existing)) attached.push("starvationProbe");
    // A witness that is already running keeps its own floors and ring sizes: retuning them mid-run
    // would make the counts on either side of this call incomparable. What is not acceptable is
    // ignoring them in silence. A caller that asked for a 40ms gap floor would otherwise read an 80ms
    // ring and conclude "no stalls happened in the window", which is how an instrument choice turns
    // into a false negative. Name everything that was not applied.
    const notApplied = [];
    if (Number.isFinite(${gapFloorMs}) && existing.gapFloorMs !== ${gapFloorMs}) {
      notApplied.push("gapFloorMs=" + existing.gapFloorMs + " (requested ${gapFloorMs} not applied)");
    }
    if (Number.isFinite(${starvationFloorMs}) && existing.starvationFloorMs !== ${starvationFloorMs}) {
      notApplied.push("starvationFloorMs=" + existing.starvationFloorMs
        + " (requested ${starvationFloorMs} not applied)");
    }
    const status = attached.length ? "upgraded:" + attached.join(",") : "present";
    return notApplied.length ? status + " | " + notApplied.join(", ") : status;
  }
  const witness = {
    gapFloorMs: ${gapFloorMs},
    timeOrigin: performance.timeOrigin,
    startedAt: performance.now(),
    frames: 0, maximumGapMs: 0, gapCount: 0, gaps: [],
    intervalLimit: ${intervalRingLimit}, intervalEvictions: 0, intervalWrite: 0,
    intervalRing: new Array(${intervalRingLimit}).fill(null),
    starvationFloorMs: ${starvationFloorMs},
    starvationLimit: ${starvationRingLimit}, starvationEvictions: 0, starvationWrite: 0,
    starvationRing: new Array(${starvationRingLimit}).fill(null),
    starvationSupport: false, starvationTicks: 0, starvationMaximumMs: 0,
    longTaskSupport: false, longTasks: [], frameSampler: wantFrameSampler,
    frameSamplerRunning: false, frameSamplerStartedAt: null,
    starvationProbeRunning: false, starvationProbeStartedAt: null,
  };
  window.__zubspaceFrameWitness = witness;
  if (typeof PerformanceObserver !== "undefined" &&
      (PerformanceObserver.supportedEntryTypes || []).includes("longtask")) {
    witness.longTaskSupport = true;
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (witness.longTasks.length < 512) {
          witness.longTasks.push({
            start: Math.round(entry.startTime), duration: Math.round(entry.duration),
          });
        }
      }
    }).observe({ type: "longtask", buffered: true });
  }
  if (wantFrameSampler) { startFrameSampler(witness, witness.startedAt); }
  if (wantStarvationProbe) { startStarvationProbe(witness); }
  return "observing";
})()`;
}

export function longTaskWitnessReadScript({ longTaskLimit = 6 } = {}) {
  return `(() => {
  const witness = window.__zubspaceFrameWitness;
  if (!witness) { return null; }
  return JSON.stringify({
    now: Math.round(performance.now()),
    startedAt: Math.round(witness.startedAt),
    frames: witness.frames,
    maximumGapMs: Math.round(witness.maximumGapMs),
    gapFloorMs: witness.gapFloorMs,
    gapCount: witness.gapCount,
    gaps: witness.gaps.slice(),
    // Oldest first, so a reader can take a tail by timestamp without sorting.
    intervals: (() => {
      const filled = Math.min(witness.intervalWrite, witness.intervalLimit);
      const start = witness.intervalWrite - filled;
      const out = [];
      for (let offset = 0; offset < filled; offset += 1) {
        const entry = witness.intervalRing[(start + offset) % witness.intervalLimit];
        if (entry) out.push(entry);
      }
      return out;
    })(),
    intervalEvictions: witness.intervalEvictions,
    starvationTicks: witness.starvationTicks,
    starvationMaximumMs: Math.round(witness.starvationMaximumMs * 10) / 10,
    starvationFloorMs: witness.starvationFloorMs,
    starvationEvictions: witness.starvationEvictions,
    starvation: (() => {
      const filled = Math.min(witness.starvationWrite, witness.starvationLimit);
      const start = witness.starvationWrite - filled;
      const out = [];
      for (let offset = 0; offset < filled; offset += 1) {
        const entry = witness.starvationRing[(start + offset) % witness.starvationLimit];
        if (entry) out.push(entry);
      }
      return out;
    })(),
    // The ring's time span, not just its contents. A reader who sees an empty entries list cannot
    // otherwise tell whether the probe watched that window and saw nothing or had already forgotten it,
    // and that distinction is the difference between two findings.
    // (No backticks in this comment: it lives inside a generated page script, and a backtick there ends
    // the template. That has happened three times, so the witness suite now parses both generated
    // scripts instead of trusting the module to load.)
    starvationRingFromMs: (() => {
      const filled = Math.min(witness.starvationWrite, witness.starvationLimit);
      const start = witness.starvationWrite - filled;
      let oldest = null;
      for (let offset = 0; offset < filled; offset += 1) {
        const entry = witness.starvationRing[(start + offset) % witness.starvationLimit];
        if (entry && (oldest === null || entry.at - entry.delayMs < oldest)) oldest = entry.at - entry.delayMs;
      }
      return oldest === null ? null : Math.round(oldest);
    })(),
    starvationRingToMs: (() => {
      const filled = Math.min(witness.starvationWrite, witness.starvationLimit);
      const start = witness.starvationWrite - filled;
      let newest = null;
      for (let offset = 0; offset < filled; offset += 1) {
        const entry = witness.starvationRing[(start + offset) % witness.starvationLimit];
        if (entry && (newest === null || entry.at > newest)) newest = entry.at;
      }
      return newest === null ? null : Math.round(newest);
    })(),
    starvationSupport: witness.starvationSupport,
    frameSamplerRunning: witness.frameSamplerRunning ?? false,
    frameSamplerStartedAt: witness.frameSamplerStartedAt ?? null,
    starvationProbeRunning: witness.starvationProbeRunning ?? false,
    starvationProbeStartedAt: witness.starvationProbeStartedAt ?? null,
    frameSampler: witness.frameSampler,
    // The page recorder's long-callback ring rides the witness read, because that read is the only
    // in-page access the WebDriver arm has: the recorder's own read path is Chrome-only, and a
    // join-freeze attribution that only worked in Chrome would be half an answer.
    pageLongFrames: (() => {
      const diagnostics = typeof window.__zubspaceFrameDiagnostics === 'function'
        ? window.__zubspaceFrameDiagnostics() : null;
      return (diagnostics && diagnostics.longFrames ? diagnostics.longFrames : []).slice(-16);
    })(),
    // Firefox's profiler stopwatch lets page code record the engine's own markers, garbage collection
    // and cycle collection included. It rides this read because the occupant of the join gap is exactly
    // the kind of work no page-level seam observes, and a claim about it needs to sit in the artifact
    // rather than in a console transcript.
    engineMarkers: (() => {
      const diagnostics = typeof window.__zubspaceFrameDiagnostics === 'function'
        ? window.__zubspaceFrameDiagnostics() : null;
      return diagnostics && diagnostics.engineMarkers
        ? diagnostics.engineMarkers : { supported: false };
    })(),
    // The task accounting rides the same read. It is here because a headline that a lane artifact
    // cannot reproduce is not evidence: the "no JavaScript task covers the block" conclusion was
    // reported from an ad-hoc console read, and the retained artifacts could not repeat it, which
    // makes the claim unverifiable by whoever inherits this item.
    pageTasks: (() => {
      const diagnostics = typeof window.__zubspaceFrameDiagnostics === 'function'
        ? window.__zubspaceFrameDiagnostics() : null;
      const tasks = (diagnostics && diagnostics.tasks) || {};
      const out = {};
      for (const key of Object.keys(tasks)) {
        const entry = tasks[key] || {};
        out[key] = { count: entry.count, sumMs: Math.round((entry.sumMs || 0) * 10) / 10,
          maxMs: Math.round((entry.maxMs || 0) * 10) / 10, maxAtMs: entry.maxAtMs };
      }
      return out;
    })(),
    pageSlowTasks: (() => {
      const diagnostics = typeof window.__zubspaceFrameDiagnostics === 'function'
        ? window.__zubspaceFrameDiagnostics() : null;
      return ((diagnostics && diagnostics.slowTasks) || []).slice(0, 24);
    })(),
    longTaskSupport: witness.longTaskSupport,
    longTaskCount: witness.longTasks.length,
    longTasks: witness.longTasks.slice().sort((a, b) => b.duration - a.duration).slice(0, ${longTaskLimit}),
  });
})()`;
}

// Starvation entries record when a self-posted `MessageChannel` task was finally handled and how
// long it waited, so the delay it describes covers `[at - delayMs, at]` rather than one instant. An
// entry overlaps a stall when those spans intersect.
// Whether a timer wait was the browser preferring rendering, or the page being unable to run anything.
// A frame COUNT cannot answer that: a 727ms wait that produced two frames was the page drawing at
// roughly 3 fps, and a count rule with a threshold of two called it "rendering normally" - the same
// freeze, labelled the opposite way depending on whether the release frame landed inside the wait. The
// quantity that matters is the rate the page drew at while the timer waited, so a wait only counts as
// scheduling when the page drew at least one frame per SCHEDULING_MAX_MS_PER_FRAME of the wait. The
// scheduling waits this instrument has to distinguish are 60-90ms with 2-4 frames (one per ~29ms, the
// page's real cadence); a stalled page draws one frame every several hundred ms.
const SCHEDULING_MAX_MS_PER_FRAME = 100;
const isSchedulingWait = (entry) => {
  if (!Number.isFinite(entry?.framesAdvanced) || !Number.isFinite(entry?.delayMs)) return false;
  if (entry.framesAdvanced <= 0) return false;
  // The wait's own length is the wrong denominator for a covering interval, whose frames could have
  // run outside the gap being asked about; the caller passes the in-gap length when it has one.
  const spanMs = Number.isFinite(entry.inGapMs) ? entry.inGapMs : entry.delayMs;
  return spanMs > 0 && entry.framesAdvanced * SCHEDULING_MAX_MS_PER_FRAME >= spanMs;
};

export function starvationInWindow(entries = [], { fromMs, toMs }) {
  const inside = (entries || [])
    .filter((entry) => Number.isFinite(entry?.at) && Number.isFinite(entry?.delayMs) &&
      entry.at - entry.delayMs <= toMs && entry.at >= fromMs)
    .sort((a, b) => b.delayMs - a.delayMs);
  // Three populations, kept apart because only the first is stall evidence. `framesAdvanced === 0`
  // means no animation callback ran while the timer waited: the thread was occupied, or rendering was
  // suspended. `framesAdvanced > 0` means the browser kept drawing and simply preferred rendering to a
  // zero-delay timer: scheduling, not occupation, and an earlier version of this probe counted those
  // intervals as a busy main thread, which produced "88ms of occupation" inside a window whose worst
  // frame gap was 32.9ms. Entries written by a page that predates the discriminator are `unknown` and
  // are never promoted into occupation - an absent observation is not evidence.
  // An entry from a page whose probe predates the discriminator stays on the reading it was made
  // with - a long timer wait with no counter-evidence - rather than silently losing an attribution an
  // archived run already made. That concession is disclosed as `unknownSamples`, and it never applies
  // to a page that did observe frames advancing: observed scheduling is never promoted to occupation.
  const speaks = (entry) => Number.isFinite(entry.framesAdvanced);
  const busy = inside.filter((entry) => !isSchedulingWait(entry));
  const waiting = inside.filter((entry) => speaks(entry) && isSchedulingWait(entry));
  const unknown = inside.filter((entry) => !speaks(entry));
  return {
    samples: inside.length,
    busySamples: busy.length,
    timerWaitSamples: waiting.length,
    unknownSamples: unknown.length,
    maximumMs: busy.length ? busy[0].delayMs : 0,
    timerWaitMaximumMs: waiting.length ? waiting[0].delayMs : 0,
    hiddenSamples: inside.filter((entry) => entry.hidden === true).length,
    entries: inside.slice(0, 4),
  };
}

// Turns a frame gap into a claim about the main thread. The readings mean different things to the
// project: the probe was delayed across the same span, so the thread was executing something else
// and the gap is a task-stall cost (whatever that work is - GC, a browser-internal task, or this
// page's own non-frame handler); the probe kept dispatching through the window and never waited, so
// the thread was free and the browser simply did not deliver a frame; or the probe cannot speak,
// because it is unsupported or the window holds no ticks of it. `windowTicks` is the tick delta for
// the whole window rather than for one gap, so the "thread was free" verdict needs a window small
// enough for that delta to describe the gap it covers.
// Entries that only straddle a gap edge describe the time outside the gap as much as inside it, and a
// single 600ms wait that began before the gap would otherwise speak for the whole gap. Gap
// attribution therefore uses containment: an entry may only classify a gap if its own interval lies
// inside the gap it is asked about. Window summaries keep the overlap rule, because a window is chosen
// by the caller and an interval that covers most of it is the caller's business.
export function starvationInsideGap(entries = [], { fromMs, toMs }) {
  // Containment alone loses the case it exists to decide. A block longer than the probe's own repost
  // interval produces one interval that starts before the gap and ends after it, so it straddles and
  // containment discards it - which is how a 713ms Firefox freeze came back with zero samples and was
  // labelled `frame-not-delivered`, the opposite verdict. So an interval that covers the gap counts as
  // well, contributing only the part inside it, and the two shapes are reported separately.
  const all = (entries || []).filter((entry) => Number.isFinite(entry?.at) && Number.isFinite(entry?.delayMs));
  const observations = [];
  for (const entry of all) {
    const start = entry.at - entry.delayMs;
    if (start >= fromMs && entry.at <= toMs) {
      observations.push({ entry, shape: "contained", inGapMs: entry.delayMs });
    } else if (start <= fromMs && entry.at >= toMs && entry.delayMs >= toMs - fromMs) {
      observations.push({ entry, shape: "covering", inGapMs: Math.round((toMs - fromMs) * 10) / 10 });
    }
  }
  observations.sort((a, b) => b.inGapMs - a.inGapMs);
  const speaks = (observation) => Number.isFinite(observation.entry.framesAdvanced);
  // The classifier needs the in-gap span as its denominator: a covering interval's own length
  // describes time outside the gap too, and dividing by it would dilute a real stall.
  const rateEntry = (observation) => (observation.shape === "covering"
    ? { ...observation.entry, inGapMs: observation.inGapMs } : observation.entry);
  const busy = observations.filter((observation) => !isSchedulingWait(rateEntry(observation)));
  const waiting = observations.filter((observation) => speaks(observation)
    && isSchedulingWait(rateEntry(observation)));
  return {
    samples: observations.length,
    busySamples: busy.length,
    timerWaitSamples: waiting.length,
    unknownSamples: observations.filter((observation) => !speaks(observation)).length,
    coveringSamples: observations.filter((observation) => observation.shape === "covering").length,
    maximumMs: busy.length ? busy[0].inGapMs : 0,
    timerWaitMaximumMs: waiting.length ? waiting[0].inGapMs : 0,
    entries: observations.slice(0, 4).map((observation) => ({
      ...observation.entry, inGapMs: observation.inGapMs, shape: observation.shape,
    })),
  };
}

export function attributeFrameGaps(gaps = [], {
  starvation = [],
  starvationSupport = false,
  windowTicks = 0,
  windowMs = 0,
  floorMs = STARVATION_FLOOR_MS,
} = {}) {
  return (gaps || []).map((gap) => {
    const seen = starvationInsideGap(starvation, { fromMs: gap.at - gap.gapMs, toMs: gap.at });
    // Publish what the classifier actually had. A reader of an artifact sees `samples: 0, entries: []`
    // next to a label like frame-not-delivered and cannot tell "the thread was observed cycling and no
    // frame arrived" from "the probe's ring had already forgotten this gap" - and those are different
    // findings. The tick density the decision used travels with the verdict.
    // `starvationObserved` is this function's own threshold made visible. Without it a reader has to
    // re-derive `windowTicks >= max(8, windowMs / 100)` from source to know whether
    // `frame-not-delivered` meant "the thread was seen cycling" or "the probe had nothing to say", and
    // an artifact whose meaning lives in the code that made it is not self-explaining.
    const observed = windowTicks > 0 && windowTicks >= Math.max(8, windowMs / 100);
    const base = { ...gap, starvation: seen, starvationWindowTicks: windowTicks,
      starvationWindowMs: Math.round(windowMs), starvationObserved: observed };
    if (!starvationSupport) {
      return { ...base, attribution: "probe-unsupported" };
    }
    if (seen.maximumMs >= floorMs) {
      return { ...base, attribution: "main-thread-busy" };
    }
    // The only long waits across this gap were timer waits while frames kept running. The thread was
    // not occupied; do not let a slow timer dress up as a stall.
    if (seen.timerWaitMaximumMs >= floorMs && seen.busySamples === 0) {
      return { ...base, attribution: "timer-starved-while-rendering" };
    }
    if (windowTicks === 0) {
      return { ...base, attribution: "probe-silent" };
    }
    // A probe task costs microseconds, so a window holding only a handful of them spent most of its
    // time inside one task. A dense window says the thread was cycling and the frame is the thing
    // that never arrived.
    if (!observed) {
      // Sparse ticks mean the probe itself was inside a long task for most of the window.
      return { ...base, attribution: "main-thread-busy" };
    }
    return { ...base, attribution: "frame-not-delivered" };
  });
}

// A long task whose `duration` grows between two reads is an unfinished task still being
// attributed, not a stall that completed - the cadence probe already reads early/late pairs for
// that reason. This helper only selects; classification stays with the caller.
export function longTasksInWindow(longTasks = [], { fromMs, toMs }) {
  return longTasks
    .filter((task) => Number.isFinite(task?.start) && task.start >= fromMs && task.start <= toMs)
    .sort((a, b) => b.duration - a.duration);
}

// Array-shaped read for call sites that embed the expression in a raw CDP command (the starvation
// witness in `combat_replay.mjs` reports `worst[0].duration` on a failure path that must not grow).
export function longTaskWitnessTopScript(limit = 4) {
  return `JSON.stringify(((window.__zubspaceFrameWitness || {}).longTasks || [])
    .slice()
    .sort((a, b) => b.duration - a.duration)
    .slice(0, ${limit}))`;
}

// Summarizes the frame intervals one window actually delivered. The gap floor only answers
// "did the page ever stop"; this answers "what did every frame cost", which is the question a
// zone merge measured under that floor still has to be answered by.
export function frameCostSummary(intervals = [], { evictions = 0 } = {}) {
  const samples = intervals.map((entry) => entry?.intervalMs).filter((ms) => Number.isFinite(ms));
  if (samples.length === 0) {
    return { samples: 0, truncated: evictions > 0 };
  }
  const sorted = samples.slice().sort((a, b) => a - b);
  const percentile = (fraction) =>
    sorted[Math.min(sorted.length - 1, Math.floor(fraction * sorted.length))];
  return {
    samples: samples.length,
    truncated: evictions > 0,
    medianMs: percentile(0.5),
    p95Ms: percentile(0.95),
    maximumMs: sorted[sorted.length - 1],
    totalMs: Math.round(samples.reduce((sum, ms) => sum + ms, 0) * 10) / 10,
    atLeast34Ms: samples.filter((ms) => ms >= 34).length,
    atLeast50Ms: samples.filter((ms) => ms >= 50).length,
    atLeast66Ms: samples.filter((ms) => ms >= 66).length,
    atLeast80Ms: samples.filter((ms) => ms >= 80).length,
  };
}

// Diffs two reads over the window they bracket. `maximumGapMs` cannot be subtracted, so it comes
// from the recorded gap timestamps; when the capped list cannot cover every gap in the window the
// result says so and keeps the page-wide maximum as a lower bound rather than inventing a number.
export function frameGapDelta(before, after) {
  if (!before || !after) {
    return { installed: false, frameWitness: before ?? after ?? null };
  }
  const fromMs = before.now;
  const toMs = after.now;
  const gaps = (after.gaps || []).filter((gap) => gap.at >= fromMs && gap.at <= toMs);
  // Exclusive at the low end: an interval that ended exactly at the read instant was already
  // behind the window when the first read was taken.
  const intervals = (after.intervals || []).filter((entry) => entry.at > fromMs && entry.at <= toMs);
  const evictions = Math.max(0, (after.intervalEvictions ?? 0) - (before.intervalEvictions ?? 0));
  const gapCount = Math.max(0, after.gapCount - before.gapCount);
  const truncated = gapCount > gaps.length;
  const worstRecordedGapMs = gaps.reduce((worst, gap) => Math.max(worst, gap.gapMs), 0);
  return {
    installed: true,
    fromMs,
    toMs,
    windowMs: toMs - fromMs,
    frames: Math.max(0, after.frames - before.frames),
    gapFloorMs: after.gapFloorMs,
    // The retained ring's time span, carried onto the window summary so a reader does not need the raw
    // read to tell an observed-quiet window from one the ring had already forgotten. It is page-scoped,
    // because the ring is, which is why it sits beside the page-wide maxima rather than the window counts.
    starvationRingFromMs: after.starvationRingFromMs ?? null,
    starvationRingToMs: after.starvationRingToMs ?? null,
    gapCount,
    gaps,
    truncated,
    maximumGapMs: truncated ? Math.max(worstRecordedGapMs, after.maximumGapMs) : worstRecordedGapMs,
    maximumGapMsIsLowerBound: truncated,
    // Every interval in the window, summarized: a merge that costs 60ms per archive shows the
    // player a hitch without ever tripping the gap floor, and this is the only view of it.
    frameCost: frameCostSummary(intervals, { evictions }),
    // Main-thread attribution. Firefox names nothing with `longtask`, so this is the only reading of
    // a Firefox freeze the page can give the project.
    starvation: (() => {
      const support = after.starvationSupport !== false && (after.starvationTicks ?? 0) > 0;
      const ticks = Math.max(0, (after.starvationTicks ?? 0) - (before.starvationTicks ?? 0));
      const inside = starvationInWindow(after.starvation || [], { fromMs, toMs });
      return {
        support,
        ticks,
        floorMs: after.starvationFloorMs ?? STARVATION_FLOOR_MS,
        pageWideMaximumMs: after.starvationMaximumMs ?? 0,
        windowMaximumMs: inside.maximumMs,
        // The two populations side by side, so "the probe waited" can never again be read as "the
        // thread was busy" without saying whether a frame ran in the interval.
        windowBusySamples: inside.busySamples,
        windowTimerWaitMaximumMs: inside.timerWaitMaximumMs,
        windowTimerWaitSamples: inside.timerWaitSamples,
        windowUnknownSamples: inside.unknownSamples,
        windowHiddenSamples: inside.hiddenSamples,
        samples: inside.samples,
        entries: inside.entries,
        evictions: Math.max(0, (after.starvationEvictions ?? 0) - (before.starvationEvictions ?? 0)),
      };
    })(),
    gapsAttributed: attributeFrameGaps(gaps, {
      starvation: after.starvation || [],
      starvationSupport: after.starvationSupport !== false && (after.starvationTicks ?? 0) > 0,
      windowTicks: Math.max(0, (after.starvationTicks ?? 0) - (before.starvationTicks ?? 0)),
      windowMs: toMs - fromMs,
      floorMs: after.starvationFloorMs ?? STARVATION_FLOOR_MS,
    }),
    pageWideMaximumGapMs: after.maximumGapMs,
    // The page-life gap list with absolute timestamps, next to `fromMs`/`toMs`, so a reader can
    // tell whether a hitch the instrument saw fell inside the window or elsewhere in page life.
    // The window-only view hid exactly that: a 578 ms Firefox hitch existed while `gapCount` was 0.
    pageWideGaps: after.gaps || [],
    pageWideGapsTruncated: after.gapCount > (after.gaps?.length ?? 0),
    // The same attribution over the page's whole gap list, which is where a join-tail freeze lives:
    // a stall that happens before a gate opens its window is invisible to `gapsAttributed`, and the
    // Firefox join freeze this instrument exists to explain is exactly that shape.
    pageWideGapsAttributed: attributeFrameGaps(after.gaps || [], {
      starvation: after.starvation || [],
      starvationSupport: after.starvationSupport !== false && (after.starvationTicks ?? 0) > 0,
      windowTicks: after.starvationTicks ?? 0,
      windowMs: (after.now ?? 0) - (after.startedAt ?? 0),
      floorMs: after.starvationFloorMs ?? STARVATION_FLOOR_MS,
    }),
    longTaskCount: after.longTaskCount,
    longTasks: longTasksInWindow(after.longTasks, { fromMs, toMs }),
  };
}

export function assertWindowFrameGaps(evidence, label) {
  if (evidence?.installed !== true) {
    throw new Error(`${label} has no frame witness installed: ${JSON.stringify(evidence ?? null)}`);
  }
  if (evidence.gapCount > 0) {
    throw new Error(`${label} recorded ${evidence.gapCount} frame gap(s) at the ` +
      `${evidence.gapFloorMs}ms floor inside a ${evidence.windowMs}ms window: ` +
      JSON.stringify({
        maximumGapMs: evidence.maximumGapMs,
        maximumGapMsIsLowerBound: evidence.maximumGapMsIsLowerBound,
        gaps: evidence.gaps,
        longTasks: evidence.longTasks,
        frames: evidence.frames,
      }));
  }
  return evidence;
}

// Attribute each frame gap against the page's own rings: which 50ms-plus work entry ran across it, and
// which task (asset-XHR handler, audio completion, module continuation) spanned it. This is the
// difference between three conclusions that all look like one number in a gap list: a callback that ran
// long, a task outside the animation callback, or the browser doing something between callbacks that
// no JavaScript owns. The third case has to be reported as itself - `busy-without-callback` - because
// an empty cover list is evidence, not a missing measurement.
//
// `work` entries are positional per the recorder's `workFields` ([kind, startMs, durationMs, ...]),
// and `slowTasks` are the published objects, so the two shapes are handled separately rather than
// assumed identical.
export function attributeGapCovers(gaps, snapshot) {
  const work = Array.isArray(snapshot && snapshot.work) ? snapshot.work : [];
  const tasks = Array.isArray(snapshot && snapshot.slowTasks) ? snapshot.slowTasks : [];
  return (Array.isArray(gaps) ? gaps : []).map((gap) => {
    const from = Number(gap && gap.at), to = from + Number(gap && gap.gapMs);
    const covers = (at, duration) => Number.isFinite(at) && Number.isFinite(duration)
      && at <= to && at + duration >= from;
    const coveringCallbacks = work.filter((entry) => covers(entry[1], entry[2]))
      .map((entry) => ({ kind: entry[0], atMs: Math.round(entry[1]), durationMs: Math.round(entry[2]) }));
    const coveringTasks = tasks.filter((entry) => covers(entry && entry.atMs, entry && entry.durationMs))
      .map((entry) => ({ kind: entry.kind, atMs: entry.atMs, durationMs: entry.durationMs }));
    const cover = coveringCallbacks.length ? "callback"
      : coveringTasks.length ? "task"
        : gap && gap.attribution === "main-thread-busy" ? "busy-without-callback"
          : (gap && gap.attribution) || "unattributed";
    return { ...gap, cover, coveringCallbacks, coveringTasks };
  });
}
