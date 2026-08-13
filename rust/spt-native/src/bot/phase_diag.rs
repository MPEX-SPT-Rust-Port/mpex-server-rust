//! Diagnostic: splits the native side of one bot generation into deserialize / generate /
//! serialize, on the request bytes `BotNativePhaseDiagTests` dumps to /tmp.

#![cfg(test)]

use std::time::Instant;

use crate::bot::bot_inventory_generator::generate_inventory;
use crate::bot::models::GenerateBotInventoryRequest;

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

#[test]
#[ignore = "diagnostic, needs /tmp/bot-request-*.json"]
fn native_phase_breakdown() {
    for role in ["assault", "usec"] {
        let path = format!("/tmp/bot-request-{role}.json");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip {role}: no {path}");
            continue;
        };

        let mut deserialize = vec![];
        let mut generate = vec![];
        let mut serialize = vec![];
        let mut out_len = 0usize;

        for run in 0..22 {
            let start = Instant::now();
            let request: GenerateBotInventoryRequest = serde_json::from_slice(&bytes).unwrap();
            let deserialized = start.elapsed().as_secs_f64() * 1000.0;

            let start = Instant::now();
            let result = generate_inventory(request).unwrap();
            let generated = start.elapsed().as_secs_f64() * 1000.0;

            let start = Instant::now();
            let json = serde_json::to_vec(&result).unwrap();
            let serialized = start.elapsed().as_secs_f64() * 1000.0;

            if run >= 2 {
                deserialize.push(deserialized);
                generate.push(generated);
                serialize.push(serialized);
                out_len = json.len();
            }
        }

        eprintln!(
            "=== {role}  request={:.2} MiB  result={:.1} KiB ===",
            bytes.len() as f64 / 1048576.0,
            out_len as f64 / 1024.0
        );
        eprintln!("  rust deserialize: {:.2} ms", median(deserialize));
        eprintln!("  rust generate:    {:.2} ms", median(generate));
        eprintln!("  rust serialize:   {:.2} ms", median(serialize));
    }
}
