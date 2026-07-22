// SPDX-License-Identifier: MIT OR Apache-2.0

//! `WDDM` GPU-spill detection: [`SpillTracker`] and the episode-based
//! [`SpillReport`].
//!
//! # What "spill" means (and what it does not)
//!
//! Under Windows / `WDDM`, the kernel's video memory manager (`VidMm`)
//! can page GPU allocations out of dedicated `VRAM` into the shared
//! system-memory budget; the resulting `PCIe` traffic is the classic
//! silent slowdown this module surfaces. Spill is **residency, not
//! commitment**: a process can *commit* GPU memory far beyond
//! dedicated `VRAM` with zero bytes actually resident in shared
//! system RAM — the normal steady state of any large `PyTorch`-style
//! process, whose caching allocator reserves a pool it has not made
//! resident. The `committed − dedicated` gap is reservation headroom,
//! not paging; inferring spill from it cries wolf on essentially every
//! serious compute workload (rhyme-mdlm dogfooding report,
//! 2026-07-19). The spill signal is the *resident shared bytes*
//! gauge — `PDH`'s `\GPU Adapter Memory(*)\Shared Usage` at the
//! adapter level, mirrored per-process by
//! `GpuProcessEntry::shared_used_bytes`.
//!
//! # The spill condition
//!
//! Shared usage has a **benign baseline** — staging/upload heaps and
//! small driver buffers live in shared memory by design — so
//! `shared > 0` alone is *not* spill. An observation counts as
//! spilling when **both** hold:
//!
//! 1. **Dedicated-resident is near its capacity** — at least
//!    [`DEFAULT_DEDICATED_THRESHOLD_PCT`]% of the adapter's dedicated
//!    `VRAM`, or the absolute figure set via
//!    [`SpillTracker::with_dedicated_threshold`] (e.g. fire at 12 GiB
//!    on a 16 GiB card — below the ≈ 13.3 GiB the 85% default works
//!    out to against the card's `DXGI` capacity — to back off
//!    *before* the slowdown starts).
//! 2. **Shared-resident has risen above its baseline** — the first
//!    observation's shared reading — by at least
//!    [`DEFAULT_SHARED_GROWTH_BYTES`] (overridable via
//!    [`SpillTracker::with_shared_growth_threshold`]).
//!
//! The co-condition suppresses benign staging-heap churn for free:
//! staging churn happens regardless of dedicated saturation, while
//! true spill only happens *at* saturation.
//!
//! **Baseline caveat:** start the tracker *before* the workload. A
//! tracker whose first observation lands mid-spill inflates the
//! baseline and under-detects from then on.
//!
//! # Transient spills: two queries, honestly named
//!
//! Live workloads flicker — spill appears for a moment, vanishes,
//! reappears — as the working set hovers at the boundary. Two methods
//! expose the two distinct facts:
//!
//! - [`SpillTracker::is_spilling`] — *instantaneous*: did the latest
//!   [`SpillTracker::observe`] meet the condition. May legitimately
//!   flip `true → false → true`.
//! - [`SpillTracker::has_spilled`] — *latched*: has **any**
//!   observation met the condition since tracking began; never reverts
//!   to `false`. What early-stop consumers almost always want — a
//!   spill that came and went still told you the budget is marginal.
//!
//! The final [`SpillReport`] records each contiguous spilling stretch
//! as a [`SpillEpisode`] rather than a single first-spill/duration
//! pair: **many short episodes** ⇒ working set marginally over budget
//! (shave the batch size); **one sustained episode** ⇒ genuinely over
//! (rethink model / precision).
//!
//! # Sampling limitation
//!
//! There is no background thread — the consumer drives observation
//! timing, and **a spill shorter than the gap between two `observe()`
//! calls is invisible**. The tracker measures at observation points,
//! full stop. `hmn spill -- <command>` (its default `--interval` is
//! 100 ms) is the offered answer when fine temporal resolution
//! matters more than in-loop integration.
//!
//! # Cross-platform honesty
//!
//! Spill into a separate shared budget is a `WDDM` architectural
//! concept. This type compiles on every platform — portable consumers
//! need no `cfg` — but only Windows can measure it:
//!
//! | Platform | Measurable? | Behaviour |
//! |---|---|---|
//! | Windows / `WDDM 2.0`+ (feature `pdh`) | yes | live `PDH` adapter query |
//! | Linux | no — normal `CUDA` OOMs rather than silently paging | [`observe`](SpillTracker::observe) is a no-op; [`is_spill_measurable`] and [`SpillTracker::has_spilled`] return `false` |
//! | macOS / Apple Silicon | no — `UMA` is one physical pool, nothing to spill *into* | same no-op contract |
//!
//! # Threading
//!
//! On Windows the tracker owns raw `PDH` handles and is therefore
//! `!Send` / `!Sync` — construct and poll it on one thread, and signal
//! out through whatever primitive the workload already uses (an
//! `AtomicBool`, a channel, a plain `break`). hypomnesis deliberately
//! has no opinion on the reaction: no callbacks, no background thread,
//! no built-in debounce (the `is_spilling` / `has_spilled` split plus
//! the episode history lets a consumer implement any debounce policy
//! in a few lines of their own code).
//!
//! # Example
//!
//! ```no_run
//! use hypomnesis::SpillTracker;
//!
//! const GIB: u64 = 1024 * 1024 * 1024;
//! let mut tracker = SpillTracker::new(0)?
//!     .with_dedicated_threshold(12 * GIB); // warn at 12, on a 16 GiB card
//!
//! for step in 0..100 {
//!     tracker.observe(format!("step_{step}"));
//!     if tracker.has_spilled() {
//!         // Latched: fires even if the spill was transient.
//!         // Drop batch size, evict KV cache, switch to CPU — consumer's choice.
//!         break;
//!     }
//!     // inference_step();
//! }
//! let report = tracker.into_report();
//! println!("{} spill episode(s)", report.episodes.len());
//! # Ok::<(), hypomnesis::HypomnesisError>(())
//! ```

use std::time::{Duration, Instant};

use crate::Result;

/// Default dedicated-saturation threshold, as a percentage of the
/// adapter's dedicated `VRAM` capacity.
///
/// An observation can only count as spilling once dedicated-resident
/// reaches this share of the card. Overridden by
/// [`SpillTracker::with_dedicated_threshold`] (which takes an
/// absolute byte figure, not a percentage).
///
/// Why 85 and not the ~95 the original design sketch assumed: `VidMm`
/// keeps real headroom below the `DXGI` nominal capacity. Live tuning
/// on the reference `RTX 5060 Ti` (2026-07-22, forced-spill fixture: a
/// 20 GiB hot working set churned on a 16 GiB card) showed
/// adapter-wide dedicated-resident **ceiling at ≈ 88.6%** of `DXGI`
/// `DedicatedVideoMemory` even under maximal pressure — a 95%
/// threshold is unreachable there and would produce systematic false
/// negatives. 85% sits safely below the measured ceiling while
/// staying far above any benign desktop load (~15–20%), and the
/// shared-growth co-condition keeps false positives suppressed.
pub const DEFAULT_DEDICATED_THRESHOLD_PCT: u64 = 85;

/// Default shared-growth margin in bytes.
///
/// Shared-resident must rise at least this far above its
/// first-observation baseline before an observation counts as
/// spilling. 256 MiB clears the benign staging-heap churn (tens of
/// MiB on the reference `RTX 5060 Ti`) and the documented
/// `KB 4490156` counter drift (~100 MiB) with margin, while sitting
/// far below any real-world spill on record (single-digit GiB).
/// Overridden by [`SpillTracker::with_shared_growth_threshold`].
pub const DEFAULT_SHARED_GROWTH_BYTES: u64 = 256 * 1024 * 1024;

/// One contiguous stretch of spilling observations.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillEpisode {
    /// Label of the first observation in the episode (the rising
    /// edge).
    pub start_label: String,
    /// Label of the first observation at which the condition no
    /// longer held (the episode's exclusive end). `None` when the
    /// tracker was still spilling at
    /// [`SpillTracker::into_report`] time.
    pub end_label: Option<String>,
    /// Highest shared-resident byte reading inside the episode.
    pub peak_shared_bytes: u64,
    /// Number of spilling observations in the episode. At least 1.
    pub observations: usize,
    /// Elapsed time from the episode's first spilling observation to
    /// its last. A single-observation episode has a duration of
    /// [`Duration::ZERO`] — the tracker cannot know how long the
    /// condition held between observation points (see the module docs
    /// on the sampling limitation).
    pub duration: Duration,
}

/// End-of-run summary produced by [`SpillTracker::into_report`].
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SpillReport {
    /// Every spill episode, in observation order. Empty ⇒ no spill
    /// was observed.
    pub episodes: Vec<SpillEpisode>,
    /// Highest dedicated-resident byte reading across all
    /// observations. `0` when nothing was observed.
    pub peak_dedicated_bytes: u64,
    /// The adapter's dedicated `VRAM` capacity in bytes (`DXGI`
    /// `DedicatedVideoMemory` — the `PDH` `GPU Adapter Memory` set
    /// carries no limit counter). `0` = unknown; renderers must not
    /// treat `0` as a real capacity.
    pub dedicated_limit_bytes: u64,
    /// Highest shared-resident byte reading across all observations.
    /// `0` when nothing was observed.
    pub peak_shared_bytes: u64,
    /// The first observation's shared-resident reading — the benign
    /// baseline the growth threshold is measured against. `0` when
    /// nothing was observed.
    pub baseline_shared_bytes: u64,
    /// Number of successful observations folded into this report.
    /// `0` on platforms where spill is not measurable (every
    /// `observe()` was a no-op).
    pub observations: usize,
    /// Whether this tracker had a live measurement source. `false`
    /// distinguishes "no spill *because none occurred*" from "no
    /// spill *because this platform cannot tell*" — `--json`
    /// consumers keying decisions off [`Self::spilled`] should check
    /// this first.
    pub measurable: bool,
}

impl SpillReport {
    /// Whether any spill episode was observed.
    #[must_use]
    pub const fn spilled(&self) -> bool {
        !self.episodes.is_empty()
    }

    /// Label of the very first spilling observation, if any.
    #[must_use]
    pub fn first_spill_label(&self) -> Option<&str> {
        // BORROW: as_str — expose the label as a borrowed view; the
        // episode keeps ownership.
        self.episodes.first().map(|e| e.start_label.as_str())
    }

    /// Sum of every episode's duration. [`Duration::ZERO`] when no
    /// spill was observed.
    #[must_use]
    pub fn total_spill_duration(&self) -> Duration {
        self.episodes.iter().map(|e| e.duration).sum()
    }

    /// The episode with the longest duration, if any. Ties resolve to
    /// the earliest such episode.
    #[must_use]
    pub fn longest_episode(&self) -> Option<&SpillEpisode> {
        // `max_by_key` returns the *last* maximum on ties; iterate in
        // reverse so ties resolve to the earliest episode instead.
        self.episodes.iter().rev().max_by_key(|e| e.duration)
    }
}

// -----------------------------------------------------------------------
// Pure fold core (platform-independent, unit-tested everywhere)
// -----------------------------------------------------------------------

/// One platform-independent reading fed to [`fold`] — produced by the
/// Windows `PDH` adapter query, or synthesized directly in unit tests.
///
/// Gated to the builds that construct it (the live Windows arm and
/// the cross-platform test harness) so non-measurable lib builds
/// don't carry dead code.
#[cfg(any(all(windows, feature = "pdh"), test))]
struct RawObservation {
    /// Adapter-wide resident dedicated `VRAM` bytes.
    dedicated_bytes: u64,
    /// Adapter-wide resident shared-system-memory bytes.
    shared_bytes: u64,
    /// Dedicated `VRAM` capacity in bytes; `0` = unknown (spill is
    /// then never flagged unless an absolute threshold override is
    /// set).
    limit_bytes: u64,
    /// Consumer-supplied label for this observation.
    label: String,
    /// Monotonic timestamp of the observation, for episode durations.
    at: Instant,
}

/// An episode that has started but not yet ended.
struct OpenEpisode {
    /// Label of the rising-edge observation.
    start_label: String,
    /// Timestamp of the rising-edge observation.
    start_at: Instant,
    /// Timestamp of the most recent spilling observation.
    last_at: Instant,
    /// Highest shared-resident reading so far in the episode.
    peak_shared: u64,
    /// Spilling observations so far in the episode.
    observations: usize,
}

impl OpenEpisode {
    /// Seal this open episode into a [`SpillEpisode`].
    fn into_episode(self, end_label: Option<String>) -> SpillEpisode {
        SpillEpisode {
            start_label: self.start_label,
            end_label,
            peak_shared_bytes: self.peak_shared,
            observations: self.observations,
            duration: self.last_at.duration_since(self.start_at),
        }
    }
}

/// Accumulated fold state behind a [`SpillTracker`].
struct TrackerState {
    /// Absolute dedicated-saturation threshold in bytes, when the
    /// consumer overrode the percentage default.
    dedicated_threshold_override: Option<u64>,
    /// Shared-growth margin in bytes (defaults to
    /// [`DEFAULT_SHARED_GROWTH_BYTES`]).
    shared_growth_bytes: u64,
    /// First observation's shared reading; `None` until the first
    /// observation lands.
    baseline_shared: Option<u64>,
    /// The currently open episode, if the latest observation was
    /// spilling.
    open: Option<OpenEpisode>,
    /// Sealed episodes, in observation order.
    episodes: Vec<SpillEpisode>,
    /// Latched spill flag — set by the first spilling observation,
    /// never cleared.
    latched: bool,
    /// Instantaneous spill flag — tracks the latest observation only.
    currently: bool,
    /// Highest dedicated-resident reading seen.
    peak_dedicated: u64,
    /// Highest shared-resident reading seen.
    peak_shared: u64,
    /// Most recent limit figure seen (static in practice — captured
    /// once per query open).
    limit_seen: u64,
    /// Successful observations folded so far.
    observations: usize,
}

impl TrackerState {
    /// Fresh state with default thresholds.
    const fn new() -> Self {
        Self {
            dedicated_threshold_override: None,
            shared_growth_bytes: DEFAULT_SHARED_GROWTH_BYTES,
            baseline_shared: None,
            open: None,
            episodes: Vec::new(),
            latched: false,
            currently: false,
            peak_dedicated: 0,
            peak_shared: 0,
            limit_seen: 0,
            observations: 0,
        }
    }
}

/// The pure per-observation transition: update peaks and counters,
/// evaluate the two-sided spill condition, and advance the episode
/// state machine (open on a rising edge, seal on a falling edge).
///
/// Gated like [`RawObservation`] — called from the live Windows arm
/// of `SpillTracker::observe` and driven directly by the
/// cross-platform unit tests.
#[cfg(any(all(windows, feature = "pdh"), test))]
fn fold(state: &mut TrackerState, obs: RawObservation) {
    state.observations = state.observations.saturating_add(1);
    state.peak_dedicated = state.peak_dedicated.max(obs.dedicated_bytes);
    state.peak_shared = state.peak_shared.max(obs.shared_bytes);
    state.limit_seen = obs.limit_bytes;
    let baseline = *state.baseline_shared.get_or_insert(obs.shared_bytes);

    let threshold = match state.dedicated_threshold_override {
        Some(t) => Some(t),
        None if obs.limit_bytes >= 100 => {
            // Integer percentage of the capacity; the sub-100-byte
            // truncation from the division is immaterial at VRAM
            // scale. Divide-first ordering also makes overflow
            // impossible.
            Some(obs.limit_bytes / 100 * DEFAULT_DEDICATED_THRESHOLD_PCT)
        }
        // EXPLICIT: capacity unknown (limit 0) or nonsensically tiny
        // (1..100 bytes, where the integer percentage collapses to 0
        // and `dedicated >= 0` would be vacuously true) and no
        // override — the saturation side of the condition cannot be
        // assessed, so the observation can never count as spilling.
        // Peaks and counters above still update.
        None => None,
    };

    let spilling = threshold.is_some_and(|t| {
        obs.dedicated_bytes >= t
            && obs.shared_bytes >= baseline.saturating_add(state.shared_growth_bytes)
    });

    state.currently = spilling;
    if spilling {
        state.latched = true;
        match state.open.as_mut() {
            Some(ep) => {
                ep.last_at = obs.at;
                ep.peak_shared = ep.peak_shared.max(obs.shared_bytes);
                ep.observations = ep.observations.saturating_add(1);
            }
            None => {
                state.open = Some(OpenEpisode {
                    start_label: obs.label,
                    start_at: obs.at,
                    last_at: obs.at,
                    peak_shared: obs.shared_bytes,
                    observations: 1,
                });
            }
        }
    } else if let Some(ep) = state.open.take() {
        state.episodes.push(ep.into_episode(Some(obs.label)));
    }
}

/// Seal any open episode and convert folded state into the final
/// report. Shared by [`SpillTracker::into_report`] and the unit tests
/// (which drive `fold` directly, without a live query — plain
/// backticks: `fold` is cfg-gated out of non-measurable lib builds,
/// so a doc link would not resolve there).
fn state_into_report(mut state: TrackerState, measurable: bool) -> SpillReport {
    if let Some(ep) = state.open.take() {
        state.episodes.push(ep.into_episode(None));
    }
    SpillReport {
        episodes: state.episodes,
        peak_dedicated_bytes: state.peak_dedicated,
        dedicated_limit_bytes: state.limit_seen,
        peak_shared_bytes: state.peak_shared,
        baseline_shared_bytes: state.baseline_shared.unwrap_or(0),
        observations: state.observations,
        measurable,
    }
}

// -----------------------------------------------------------------------
// Public tracker
// -----------------------------------------------------------------------

/// Fold-over-observations `WDDM` spill tracker — see the [module
/// docs][self] for semantics, the spill condition, and the
/// cross-platform contract.
///
/// The consumer drives observation timing by calling
/// [`observe`](Self::observe) inside their existing loop;
/// [`is_spilling`](Self::is_spilling) / [`has_spilled`](Self::has_spilled)
/// are cheap queries over already-collected state (no fresh sample);
/// [`into_report`](Self::into_report) seals the episode history.
///
/// On Windows this type holds raw `PDH` handles and is therefore
/// `!Send` / `!Sync` — construct it on the thread that polls.
pub struct SpillTracker {
    /// Accumulated fold state (platform-independent).
    state: TrackerState,
    /// Long-lived `PDH` adapter query. `None` = spill not measurable
    /// on this system (counter set absent / adapter invisible), the
    /// graceful sibling of the non-Windows stub.
    #[cfg(all(windows, feature = "pdh"))]
    live: Option<crate::gpu::pdh::AdapterMemQuery>,
}

impl SpillTracker {
    /// Construct a tracker for the GPU at `device_index`
    /// (`NVML`-canonical ordering, matching
    /// [`crate::device_info`]).
    ///
    /// Succeeds on every platform: where spill is not measurable
    /// (Linux, macOS, Windows without the `pdh` feature or the
    /// `GPU Adapter Memory` counter set), the tracker constructs in
    /// its non-measurable state — [`observe`](Self::observe) becomes
    /// a no-op and [`has_spilled`](Self::has_spilled) stays `false` —
    /// so portable consumers need no platform `cfg`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::HypomnesisError::Pdh`] (Windows + `pdh` only)
    /// when the `DXGI` walk cannot locate `device_index` or the `PDH`
    /// query cannot be opened — hard failures, distinct from the
    /// graceful non-measurable degradation above.
    #[allow(clippy::missing_const_for_fn)] // const only on non-Windows builds (body collapses)
    pub fn new(device_index: u32) -> Result<Self> {
        #[cfg(all(windows, feature = "pdh"))]
        {
            let live = crate::gpu::pdh::AdapterMemQuery::open(device_index)?;
            Ok(Self {
                state: TrackerState::new(),
                live,
            })
        }
        #[cfg(not(all(windows, feature = "pdh")))]
        {
            // EXPLICIT: no measurement source on this platform; the
            // tracker still constructs (portable-consumer contract)
            // and stays non-measurable.
            let _ = device_index;
            Ok(Self {
                state: TrackerState::new(),
            })
        }
    }

    /// Override the dedicated-saturation threshold with an absolute
    /// byte figure (replacing the
    /// [`DEFAULT_DEDICATED_THRESHOLD_PCT`]% -of-capacity default).
    /// Pins the threshold independent of the capacity heuristic —
    /// set it *below* the default (e.g. 12 GiB on a 16 GiB card,
    /// where 85% of the `DXGI` capacity ≈ 13.3 GiB) to back off
    /// proactively before the slowdown starts, or above it to
    /// tolerate more pressure.
    #[must_use]
    pub const fn with_dedicated_threshold(mut self, bytes: u64) -> Self {
        self.state.dedicated_threshold_override = Some(bytes);
        self
    }

    /// Override the shared-growth margin (default
    /// [`DEFAULT_SHARED_GROWTH_BYTES`]): how far shared-resident must
    /// rise above its first-observation baseline before an
    /// observation counts as spilling.
    #[must_use]
    pub const fn with_shared_growth_threshold(mut self, bytes: u64) -> Self {
        self.state.shared_growth_bytes = bytes;
        self
    }

    /// Take one observation, labelled for the episode history (e.g.
    /// `"step_42"`, or a timestamp).
    ///
    /// Infallible by design — the polling loop in the module example
    /// calls it bare. A failed `PDH` sample is a *skipped*
    /// observation (traced under the `debug-output` feature), not an
    /// error: measurement hiccups must not disturb the workload being
    /// measured. On non-measurable platforms this is a no-op and the
    /// label is discarded.
    #[allow(clippy::missing_const_for_fn)] // const only on non-Windows builds (body collapses)
    #[allow(clippy::needless_pass_by_value)] // Into<String> by value is the intended call shape
    pub fn observe<S: Into<String>>(&mut self, label: S) {
        #[cfg(all(windows, feature = "pdh"))]
        {
            let Some(live) = self.live.as_mut() else {
                return;
            };
            match live.sample() {
                Ok(s) => fold(
                    &mut self.state,
                    RawObservation {
                        dedicated_bytes: s.dedicated_bytes,
                        shared_bytes: s.shared_bytes,
                        limit_bytes: s.limit_bytes,
                        label: label.into(),
                        at: Instant::now(),
                    },
                ),
                #[cfg(feature = "debug-output")]
                Err(e) => eprintln!("[spill debug] adapter sample failed: {e}"),
                // EXPLICIT: skipped observation — see the doc comment;
                // the tracker keeps its previous state.
                #[cfg(not(feature = "debug-output"))]
                Err(_) => {}
            }
        }
        #[cfg(not(all(windows, feature = "pdh")))]
        {
            // EXPLICIT: nothing to measure on this platform; no
            // observation is recorded.
            let _ = label;
        }
    }

    /// *Instantaneous* spill state: did the **latest** observation
    /// meet the spill condition. May legitimately flip
    /// `true → false → true` as the working set hovers at the
    /// boundary (see the module docs on transient spills). Always
    /// `false` where spill is not measurable.
    #[must_use]
    pub const fn is_spilling(&self) -> bool {
        self.state.currently
    }

    /// *Latched* spill state: has **any** observation met the spill
    /// condition since tracking began; never reverts to `false`.
    /// What early-stop consumers almost always want. Always `false`
    /// where spill is not measurable.
    #[must_use]
    pub const fn has_spilled(&self) -> bool {
        self.state.latched
    }

    /// Whether **this instance** has a live measurement source.
    /// `false` on Linux / macOS, on Windows builds without the `pdh`
    /// feature, and on Windows systems whose `GPU Adapter Memory`
    /// counter set is absent. See also the free function
    /// [`is_spill_measurable`] for the system-level probe.
    #[allow(clippy::missing_const_for_fn)] // const only on non-Windows builds (body collapses)
    #[must_use]
    pub fn is_measurable(&self) -> bool {
        #[cfg(all(windows, feature = "pdh"))]
        {
            self.live.is_some()
        }
        #[cfg(not(all(windows, feature = "pdh")))]
        {
            false
        }
    }

    /// Seal the episode history (an episode still open at this point
    /// gets `end_label: None`) and return the end-of-run
    /// [`SpillReport`].
    #[must_use]
    pub fn into_report(self) -> SpillReport {
        let measurable = self.is_measurable();
        state_into_report(self.state, measurable)
    }
}

/// System-level capability probe: can this platform measure `WDDM`
/// spill at all?
///
/// `true` only on Windows with the `pdh` feature and the
/// `GPU Adapter Memory` counter set registered with at least one
/// instance (`WDDM 2.0`+); `false` on Linux (normal `CUDA` gets an
/// OOM rather than silent paging; `NVML` has no shared-residency
/// field) and macOS (`UMA` — a single physical pool, nothing to
/// spill *into*). Portable consumers can use this to skip an
/// early-stop polling path entirely rather than polling a tracker
/// that will never fire.
///
/// This is a *probe*, not a guarantee: a specific
/// [`SpillTracker::new`] can still come up non-measurable (or `Err`)
/// when its particular `device_index` doesn't resolve — e.g. the
/// adapter's `LUID` matches no enumerated instance, or the `DXGI`
/// walk finds no adapter at that index.
/// [`SpillTracker::is_measurable`] is the per-instance truth.
#[allow(clippy::missing_const_for_fn)] // const only on non-Windows builds (body collapses)
#[must_use]
pub fn is_spill_measurable() -> bool {
    #[cfg(all(windows, feature = "pdh"))]
    {
        crate::gpu::pdh::adapter_counter_set_available()
    }
    #[cfg(not(all(windows, feature = "pdh")))]
    {
        false
    }
}

// -----------------------------------------------------------------------
// test-helpers builder (synthetic SpillReport fixtures)
// -----------------------------------------------------------------------

/// Builder for synthetic [`SpillReport`] values in downstream tests.
///
/// `SpillReport` is `#[non_exhaustive]`, so struct-literal
/// construction is unavailable outside this crate — including to the
/// `hmn` binary's own formatter tests, the first consumer of this
/// builder.
///
/// Available only with `features = ["test-helpers"]`; production code
/// must never enable it.
///
/// ```
/// # #[cfg(feature = "test-helpers")]
/// # {
/// use std::time::Duration;
/// use hypomnesis::SpillReport;
///
/// let report = SpillReport::builder()
///     .measurable(true)
///     .observations(42)
///     .episode("+12.4s", Some("+16.2s"), 4 * 1_024 * 1_024 * 1_024, 38, Duration::from_millis(3_800))
///     .build();
/// assert!(report.spilled());
/// # }
/// ```
#[cfg(feature = "test-helpers")]
#[derive(Debug, Clone, Default)]
pub struct SpillReportBuilder {
    /// Pending [`SpillReport::episodes`] value, defaults to empty.
    episodes: Vec<SpillEpisode>,
    /// Pending [`SpillReport::peak_dedicated_bytes`] value, defaults to `0`.
    peak_dedicated_bytes: u64,
    /// Pending [`SpillReport::dedicated_limit_bytes`] value, defaults to `0`.
    dedicated_limit_bytes: u64,
    /// Pending [`SpillReport::peak_shared_bytes`] value, defaults to `0`.
    peak_shared_bytes: u64,
    /// Pending [`SpillReport::baseline_shared_bytes`] value, defaults to `0`.
    baseline_shared_bytes: u64,
    /// Pending [`SpillReport::observations`] value, defaults to `0`.
    observations: usize,
    /// Pending [`SpillReport::measurable`] value, defaults to `false`.
    measurable: bool,
}

#[cfg(feature = "test-helpers")]
impl SpillReport {
    /// Start a builder for constructing synthetic `SpillReport` values
    /// in downstream tests.
    ///
    /// Available only with `features = ["test-helpers"]`. See
    /// [`SpillReportBuilder`] for the full discussion.
    #[must_use]
    pub fn builder() -> SpillReportBuilder {
        SpillReportBuilder::default()
    }
}

#[cfg(feature = "test-helpers")]
impl SpillReportBuilder {
    /// Set the highest dedicated-resident reading in bytes.
    #[must_use]
    pub const fn peak_dedicated_bytes(mut self, bytes: u64) -> Self {
        self.peak_dedicated_bytes = bytes;
        self
    }

    /// Set the dedicated capacity in bytes (`0` = unknown).
    #[must_use]
    pub const fn dedicated_limit_bytes(mut self, bytes: u64) -> Self {
        self.dedicated_limit_bytes = bytes;
        self
    }

    /// Set the highest shared-resident reading in bytes.
    #[must_use]
    pub const fn peak_shared_bytes(mut self, bytes: u64) -> Self {
        self.peak_shared_bytes = bytes;
        self
    }

    /// Set the first-observation shared baseline in bytes.
    #[must_use]
    pub const fn baseline_shared_bytes(mut self, bytes: u64) -> Self {
        self.baseline_shared_bytes = bytes;
        self
    }

    /// Set the number of successful observations.
    #[must_use]
    pub const fn observations(mut self, n: usize) -> Self {
        self.observations = n;
        self
    }

    /// Set whether the synthetic tracker had a live measurement source.
    #[must_use]
    pub const fn measurable(mut self, measurable: bool) -> Self {
        self.measurable = measurable;
        self
    }

    /// Append one spill episode (`end_label: None` = still spilling at
    /// report time).
    #[must_use]
    pub fn episode(
        mut self,
        start_label: &str,
        end_label: Option<&str>,
        peak_shared_bytes: u64,
        observations: usize,
        duration: Duration,
    ) -> Self {
        self.episodes.push(SpillEpisode {
            // BORROW: to_owned — the builder call site keeps string
            // literals; the episode owns its labels.
            start_label: start_label.to_owned(),
            end_label: end_label.map(str::to_owned),
            peak_shared_bytes,
            observations,
            duration,
        });
        self
    }

    /// Consume the builder and produce the configured `SpillReport`.
    ///
    /// Unset fields take the documented defaults.
    #[must_use]
    pub fn build(self) -> SpillReport {
        SpillReport {
            episodes: self.episodes,
            peak_dedicated_bytes: self.peak_dedicated_bytes,
            dedicated_limit_bytes: self.dedicated_limit_bytes,
            peak_shared_bytes: self.peak_shared_bytes,
            baseline_shared_bytes: self.baseline_shared_bytes,
            observations: self.observations,
            measurable: self.measurable,
        }
    }
}

// -----------------------------------------------------------------------
// Tests (pure fold core — no FFI, run on every platform)
// -----------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    // INDEX: fixture-driven assertions on episode lists whose lengths
    // are asserted immediately beforehand.
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// 16 GiB reference capacity.
    const LIMIT: u64 = 16 * 1024 * 1024 * 1024;
    /// A dedicated reading safely above the 85% default threshold.
    const SATURATED: u64 = LIMIT - 1024;
    /// A dedicated reading safely below the 85% default threshold.
    const RELAXED: u64 = LIMIT / 2;
    /// Benign shared baseline (~134 MiB, the live idle reading on the
    /// reference card).
    const BASELINE: u64 = 140_533_760;
    /// A shared reading clearly past baseline + growth margin.
    const SPILLED_SHARED: u64 = BASELINE + DEFAULT_SHARED_GROWTH_BYTES + 512 * 1024 * 1024;

    fn obs(label: &str, at: Instant, dedicated: u64, shared: u64) -> RawObservation {
        RawObservation {
            dedicated_bytes: dedicated,
            shared_bytes: shared,
            limit_bytes: LIMIT,
            label: label.to_owned(),
            at,
        }
    }

    /// Drive a sequence of `(label, dedicated, shared)` triples
    /// through a fresh state, 1 s apart.
    fn drive(seq: &[(&str, u64, u64)]) -> TrackerState {
        let mut state = TrackerState::new();
        let t0 = Instant::now();
        for (i, (label, dedicated, shared)) in seq.iter().enumerate() {
            // CAST: usize → u64, loop index bounded by the fixture
            // length (single digits).
            #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
            let at = t0 + Duration::from_secs(i as u64);
            fold(&mut state, obs(label, at, *dedicated, *shared));
        }
        state
    }

    #[test]
    fn fold_flicker_three_episodes() {
        // Three rise/fall cycles ⇒ exactly 3 episodes; the latch stays
        // set while the instantaneous flag tracks the latest
        // observation.
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED), // episode 1 opens
            ("t2", RELAXED, BASELINE),         // episode 1 closes
            ("t3", SATURATED, SPILLED_SHARED), // episode 2 opens
            ("t4", RELAXED, BASELINE),         // episode 2 closes
            ("t5", SATURATED, SPILLED_SHARED), // episode 3 opens
            ("t6", RELAXED, BASELINE),         // episode 3 closes
        ]);
        assert!(state.latched);
        assert!(!state.currently); // latest observation is not spilling
        let report = state_into_report(state, true);
        assert_eq!(report.episodes.len(), 3);
        assert!(report.spilled());
        assert_eq!(report.first_spill_label(), Some("t1"));
        assert_eq!(report.episodes[0].end_label.as_deref(), Some("t2"));
        assert_eq!(report.episodes[2].end_label.as_deref(), Some("t6"));
    }

    #[test]
    fn fold_benign_baseline_zero_episodes() {
        // Dedicated saturated but shared flat at its baseline: the
        // staging-heap case — NOT spill.
        let state = drive(&[
            ("t0", SATURATED, BASELINE),
            ("t1", SATURATED, BASELINE + 1024), // wiggle below the margin
            ("t2", SATURATED, BASELINE),
        ]);
        assert!(!state.latched);
        assert!(!state.currently);
        let report = state_into_report(state, true);
        assert!(report.episodes.is_empty());
        assert!(!report.spilled());
    }

    #[test]
    fn fold_commit_gap_zero_episodes() {
        // The rhyme-mdlm false-positive fixture: dedicated below the
        // threshold, shared at baseline. Committed bytes are not an
        // input at all — the commit gap can never fire this condition.
        let state = drive(&[("t0", RELAXED, BASELINE), ("t1", RELAXED, BASELINE)]);
        assert!(!state.latched);
        let report = state_into_report(state, true);
        assert!(report.episodes.is_empty());
    }

    #[test]
    fn fold_still_spilling_end_label_none() {
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", SATURATED, SPILLED_SHARED),
        ]);
        assert!(state.currently);
        let report = state_into_report(state, true);
        assert_eq!(report.episodes.len(), 1);
        assert_eq!(report.episodes[0].end_label, None);
        assert_eq!(report.episodes[0].observations, 2);
    }

    #[test]
    fn fold_latched_survives_recovery() {
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", RELAXED, BASELINE),
        ]);
        assert!(state.latched); // has_spilled: sticky
        assert!(!state.currently); // is_spilling: tracks the latest
    }

    #[test]
    fn fold_baseline_from_first_observation() {
        // A tracker started mid-churn takes its first reading as the
        // baseline: growth is measured from there.
        let elevated = BASELINE + 512 * 1024 * 1024;
        let state = drive(&[
            ("t0", RELAXED, elevated), // baseline = elevated
            ("t1", SATURATED, elevated + DEFAULT_SHARED_GROWTH_BYTES),
        ]);
        assert!(state.latched); // fires at elevated + margin
        let report = state_into_report(state, true);
        assert_eq!(report.baseline_shared_bytes, elevated);
    }

    #[test]
    fn fold_growth_boundary_fires_at_baseline_plus_margin() {
        // Boundary semantics: >= baseline + margin fires; one byte
        // below does not.
        let below = drive(&[
            ("t0", SATURATED, BASELINE),
            ("t1", SATURATED, BASELINE + DEFAULT_SHARED_GROWTH_BYTES - 1),
        ]);
        assert!(!below.latched);

        let at_margin = drive(&[
            ("t0", SATURATED, BASELINE),
            ("t1", SATURATED, BASELINE + DEFAULT_SHARED_GROWTH_BYTES),
        ]);
        assert!(at_margin.latched);
    }

    #[test]
    fn fold_zero_limit_never_spills() {
        // Capacity unknown (limit 0) and no override: the saturation
        // side cannot be assessed, so nothing ever counts as spilling —
        // but peaks and counters still update.
        let mut state = TrackerState::new();
        let t0 = Instant::now();
        fold(
            &mut state,
            RawObservation {
                dedicated_bytes: u64::MAX,
                shared_bytes: u64::MAX,
                limit_bytes: 0,
                label: "t0".to_owned(),
                at: t0,
            },
        );
        assert!(!state.latched);
        assert_eq!(state.peak_dedicated, u64::MAX);
        assert_eq!(state.observations, 1);
    }

    #[test]
    fn fold_sub_100_byte_limit_never_spills() {
        // Degenerate capacity (1..100 bytes): the integer percentage
        // collapses to 0 and `dedicated >= 0` would be vacuously
        // true — guarded to "cannot assess saturation" instead.
        let mut state = TrackerState::new();
        fold(
            &mut state,
            RawObservation {
                dedicated_bytes: u64::MAX,
                shared_bytes: u64::MAX,
                limit_bytes: 99,
                label: "t0".to_owned(),
                at: Instant::now(),
            },
        );
        assert!(!state.latched);
    }

    #[test]
    fn fold_custom_dedicated_threshold_override() {
        // Override at 12 GiB on the 16 GiB card: a 12 GiB dedicated
        // reading — below the 85% default of ~13.6 GiB, so it does
        // NOT fire without the override — now fires, given shared
        // growth. Both halves asserted so the test discriminates the
        // override path.
        let twelve_gib = 12 * 1024 * 1024 * 1024;
        assert!(twelve_gib < LIMIT / 100 * DEFAULT_DEDICATED_THRESHOLD_PCT);

        let without_override = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", twelve_gib, SPILLED_SHARED),
        ]);
        assert!(!without_override.latched);

        let mut state = TrackerState::new();
        state.dedicated_threshold_override = Some(twelve_gib);
        let t0 = Instant::now();
        fold(&mut state, obs("t0", t0, RELAXED, BASELINE));
        fold(
            &mut state,
            obs(
                "t1",
                t0 + Duration::from_secs(1),
                twelve_gib,
                SPILLED_SHARED,
            ),
        );
        assert!(state.latched);
    }

    #[test]
    fn fold_single_observation_episode_zero_duration() {
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", RELAXED, BASELINE),
        ]);
        let report = state_into_report(state, true);
        assert_eq!(report.episodes.len(), 1);
        // One spilling observation: the tracker cannot know how long
        // the condition held between observation points.
        assert_eq!(report.episodes[0].duration, Duration::ZERO);
        assert_eq!(report.episodes[0].observations, 1);
    }

    #[test]
    fn fold_peaks_and_observation_counts() {
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", RELAXED, BASELINE),
        ]);
        assert_eq!(state.observations, 3);
        assert_eq!(state.peak_dedicated, SATURATED);
        assert_eq!(state.peak_shared, SPILLED_SHARED);
        let report = state_into_report(state, true);
        assert_eq!(report.observations, 3);
        assert_eq!(report.peak_dedicated_bytes, SATURATED);
        assert_eq!(report.peak_shared_bytes, SPILLED_SHARED);
        assert_eq!(report.baseline_shared_bytes, BASELINE);
        assert_eq!(report.dedicated_limit_bytes, LIMIT);
        assert!(report.measurable);
    }

    #[test]
    fn report_total_spill_duration_sums_episodes() {
        // Two episodes of 2 s each (three spilling observations 1 s
        // apart = 2 s per episode).
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", SATURATED, SPILLED_SHARED),
            ("t3", SATURATED, SPILLED_SHARED),
            ("t4", RELAXED, BASELINE),
            ("t5", SATURATED, SPILLED_SHARED),
            ("t6", SATURATED, SPILLED_SHARED),
            ("t7", SATURATED, SPILLED_SHARED),
            ("t8", RELAXED, BASELINE),
        ]);
        let report = state_into_report(state, true);
        assert_eq!(report.episodes.len(), 2);
        assert_eq!(report.total_spill_duration(), Duration::from_secs(4));
    }

    #[test]
    fn report_longest_episode() {
        // Episode 1: single observation (0 s). Episode 2: two
        // observations (1 s) — the longest.
        let state = drive(&[
            ("t0", RELAXED, BASELINE),
            ("t1", SATURATED, SPILLED_SHARED),
            ("t2", RELAXED, BASELINE),
            ("t3", SATURATED, SPILLED_SHARED),
            ("t4", SATURATED, SPILLED_SHARED),
            ("t5", RELAXED, BASELINE),
        ]);
        let report = state_into_report(state, true);
        let longest = report.longest_episode().unwrap();
        assert_eq!(longest.start_label, "t3");
        assert_eq!(longest.duration, Duration::from_secs(1));
    }

    #[test]
    fn report_first_spill_label_none_when_no_spill() {
        let state = drive(&[("t0", RELAXED, BASELINE)]);
        let report = state_into_report(state, true);
        assert_eq!(report.first_spill_label(), None);
        assert_eq!(report.total_spill_duration(), Duration::ZERO);
        assert!(report.longest_episode().is_none());
    }

    #[cfg(not(all(windows, feature = "pdh")))]
    #[test]
    fn tracker_unmeasurable_observe_is_noop() {
        // Exercises the real non-Windows stub: construction succeeds,
        // observe records nothing, every query stays false.
        let mut tracker = SpillTracker::new(0).unwrap();
        assert!(!tracker.is_measurable());
        tracker.observe("t0");
        assert!(!tracker.is_spilling());
        assert!(!tracker.has_spilled());
        let report = tracker.into_report();
        assert_eq!(report.observations, 0);
        assert!(!report.measurable);
        assert!(!report.spilled());
    }
}
