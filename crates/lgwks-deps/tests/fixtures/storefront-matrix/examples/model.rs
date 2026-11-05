//! A model client on ring-backed TLS resolves providers offline: no key, no network.
//!
//! `sim <seed> 1` races the process's first TLS install instead: a seeded
//! number of threads (100 or 1,000), each after a seeded spin, all call
//! `install_ring` at once, and every one must be told ring is installed before
//! the journey runs on the provider they agreed on.

#[path = "support/boot.rs"]
mod boot;
#[path = "support/sim.rs"]
mod sim;

use lgwks_deps::genai::Client;
use lgwks_deps::genai::adapter::AdapterKind;
use lgwks_deps::genai::chat::{ChatMessage, ChatRequest};
use lgwks_std::seeded::Seeded;

/// The concurrency tiers the double-clicker races at.
const RACERS: [usize; 2] = [100, 1_000];

fn main() {
    let raced = match sim::seeds() {
        Some(range) => race(range),
        None => true,
    };
    if !(raced && boot::run_with_tls(journey())) {
        std::process::exit(1);
    }
}

/// Every thread of the first seed's tier calls `install_ring` at once; the
/// race is decided once per process, so later seeds re-race an installed one.
fn race(range: std::ops::Range<u64>) -> bool {
    let mut trace = sim::Trace::new();
    for seed in range {
        let mut rng = Seeded::from_seed(seed);
        let Ok(tier) = rng.index(RACERS.len()) else {
            return false;
        };
        let Some(&racers) = RACERS.get(tier) else {
            return false;
        };
        let spins: Vec<u64> = (0..racers).filter_map(|_| rng.below(4_096).ok()).collect();
        let agreed = std::thread::scope(|scope| {
            let handles: Vec<_> = spins
                .iter()
                .map(|&spin| {
                    scope.spawn(move || {
                        for _ in 0..spin {
                            std::hint::spin_loop();
                        }
                        lgwks_deps::tls::install_ring()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(std::thread::ScopedJoinHandle::join)
                .filter(|joined| matches!(joined, Ok(Ok(()))))
                .count()
        });
        if spins.len() != racers || agreed != racers {
            return false;
        }
        let Ok(agreed) = u64::try_from(agreed) else {
            return false;
        };
        let tier_name = if racers == 1_000 {
            "thousand_agreed"
        } else {
            "hundred_agreed"
        };
        trace.record(tier_name, agreed);
    }
    trace.report()
}

async fn journey() -> bool {
    let Ok(client) = Client::builder().build() else {
        return false;
    };
    let mut adapters = Vec::new();
    for model in [
        "deepseek-chat",
        "claude-opus-5-5",
        "gpt-5.5",
        "anthropic::opus",
    ] {
        let Ok(target) = client.resolve_service_target(model).await else {
            return false;
        };
        adapters.push(target.model.adapter_kind);
    }
    let request = ChatRequest::new(vec![
        ChatMessage::system("Answer with one word."),
        ChatMessage::user("ping"),
    ]);
    adapters
        == [
            AdapterKind::DeepSeek,
            AdapterKind::Anthropic,
            AdapterKind::OpenAIResp,
            AdapterKind::Anthropic,
        ]
        && request.messages.len() == 2
}
