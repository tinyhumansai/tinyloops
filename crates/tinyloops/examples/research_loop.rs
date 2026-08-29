//! The shipped preset, driven end to end, offline.
//!
//! It assembles a `research_loop` over the reference seams — a fixed
//! decomposition, an inline specialist dispatcher, the two evaluation arms —
//! and drives it to a terminal state, printing the loop's own vocabulary as it
//! goes. It needs no credentials, no network, and no wall clock, so it runs
//! identically in CI and on a laptop.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p tinyloops --example research_loop
//! ```
//!
//! What it prints, in order: every pass boundary, every step's entry and exit,
//! every arm, the merge, the verdict, the route each pass took and the counters
//! it was taken on, and the closing report. An accuracy figure with no cost
//! beside it is not a comparable result, so the summary ends with what the run
//! spent rather than only with how it came out.

use std::sync::Arc;

use tinyloops::{
    Artifact, DelegateSet, Driven, Error, FixedPlan, Inline, LineSink, Preset, Recorder,
    SOLVED_MARKER, Scripted, research_loop,
};

fn main() -> Result<(), Error> {
    let delegates = DelegateSet::of(["prover", "refuter"]);

    // A decomposition stated up front. In a deployment this is a model behind
    // the `Decompose` seam; here it is a value, which is what makes the run
    // reproducible.
    let plan = Arc::new(FixedPlan::of([
        (
            "bound-the-error",
            "bound the error term",
            "a proved bound on disk",
        ),
        (
            "check-the-edge-case",
            "check the n = 0 edge case",
            "a counterexample or a proof there is none",
        ),
    ]));

    // The specialists, scripted. The prover's queue has three entries and the
    // run takes two passes, because `research` briefs the first declared
    // specialist once before the loop starts and consumes an entry doing it.
    // The prover then finds nothing on the first pass and succeeds on the
    // second, so the run exercises a real routing decision rather than solving
    // on pass zero.
    let specialists = Arc::new(Inline::of(
        delegates.clone(),
        [
            (
                "prover".to_owned(),
                vec![
                    Scripted::Answers {
                        reply: "the second term is where the difficulty is".to_owned(),
                        artifacts: Vec::new(),
                    },
                    Scripted::Answers {
                        reply: "no bound yet; the second term resists".to_owned(),
                        artifacts: vec![Artifact::new("attempt-1.md", "the failed approach")],
                    },
                    Scripted::Answers {
                        reply: format!("{SOLVED_MARKER}: the bound holds for all n"),
                        artifacts: vec![Artifact::new("bound.md", "the proof")],
                    },
                ],
            ),
            (
                "refuter".to_owned(),
                vec![Scripted::Capped {
                    artifacts: vec![Artifact::new("search.log", "the partial search")],
                }],
            ),
        ],
    ));

    let assembled = research_loop(
        "bound the error term in the partial sum",
        Preset::Balanced,
        delegates,
        plan,
        specialists,
    )?;

    println!("preset:    {}", assembled.preset());
    println!("stuck at:  {}", assembled.profile().thresholds.stuck);
    println!("signature: {}\n", assembled.signature()?.as_str());

    // A recorder over the console sink: one line per event, in one ordered
    // stream. `Recorder::child` would give a delegated run its own label while
    // sharing this journal, which is what makes a per-role view possible
    // without a second writer.
    let recorder = Recorder::new("run", Arc::new(LineSink::new(std::io::stdout())));
    let driven: Driven = assembled.drive(&recorder)?;

    println!("\n=== report ===\n{}", driven.answer());
    println!("outcome:  {:?}", driven.outcome);
    println!(
        "routes:   {}",
        driven
            .routes
            .iter()
            .map(|route| route.as_str())
            .collect::<Vec<_>>()
            .join(" -> ")
    );
    println!("passes:   {}", driven.state.passes);
    println!("attempts: {}", driven.state.attempts);
    println!("banked:   {}", driven.state.banked);
    println!("bound:    {:?}", driven.bound);
    println!("events:   {}", recorder.journal().len());

    Ok(())
}
