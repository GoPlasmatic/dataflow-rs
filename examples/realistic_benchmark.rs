//! # Realistic Throughput Benchmark
//!
//! Mirrors the shape of a real dataflow-rs workload: an ISO 20022 pacs.008-
//! style inbound payload (4–5 levels deep, ~80 fields), transformed into a
//! SwiftMT-103-style outbound shape via ~25 map mappings, then validated
//! against ~12 rules. The map task exercises a realistic mix of operators:
//! `var`, `cat`, `if`, arithmetic, `!!`, `==`, `substr`, `length`.
//!
//! Compare numbers against `benchmark.rs` (the 9-eval trivial workload) to
//! see how the per-message framework overhead (UUID, audit drop, message
//! construction) reweights when real eval work is added.
//!
//! Run with: `cargo run --example realistic_benchmark --release`

use dataflow_rs::{Engine, Message, Workflow};
use datavalue::OwnedDataValue;
use futures::future::join_all;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOTAL_MESSAGES: usize = 500_000;
const WARMUP_MESSAGES: usize = 5_000;

struct LatencyStats {
    measurements: Vec<Duration>,
}

impl LatencyStats {
    fn new() -> Self {
        Self {
            measurements: Vec::with_capacity(TOTAL_MESSAGES),
        }
    }

    fn add(&mut self, d: Duration) {
        self.measurements.push(d);
    }

    fn percentiles(&mut self) -> (Duration, Duration, Duration, Duration, Duration) {
        self.measurements.sort_unstable();
        let n = self.measurements.len();
        if n == 0 {
            let z = Duration::ZERO;
            return (z, z, z, z, z);
        }
        (
            self.measurements[n * 50 / 100],
            self.measurements[n * 90 / 100],
            self.measurements[n * 95 / 100],
            self.measurements[n * 99 / 100],
            self.measurements[std::cmp::min(n * 999 / 1000, n - 1)],
        )
    }

    fn average(&self) -> Duration {
        if self.measurements.is_empty() {
            return Duration::ZERO;
        }
        let sum: Duration = self.measurements.iter().sum();
        sum / self.measurements.len() as u32
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("REALISTIC ENGINE BENCHMARK (ISO 20022 -> SwiftMT-shaped workload)");
    println!("=================================================================");
    println!("Total messages: {}", TOTAL_MESSAGES);
    println!("CPU cores: {}", num_cpus::get());
    println!("Tokio worker threads: {}", num_cpus::get());
    println!();

    // -- Workflow: realistic transformation + validation ----------------------
    // 1 parse_json (payload -> data.input)
    // 1 map task: 25 mappings exercising var/cat/if/arithmetic/substr/length
    // 1 validation task: 12 rules
    let workflow = build_workflow();

    let engine = Arc::new(Engine::new(vec![workflow], None).unwrap());

    // -- Sample payload: pacs.008-shaped, ~80 fields, 4-5 levels deep ---------
    // Convert to `OwnedDataValue` ONCE at startup and share via `Arc`. Per-
    // message construction goes through `Message::new(Arc::clone(...))`
    // — a refcount bump, no serde_json clone or `OwnedDataValue::from(&Value)`
    // walk. This is what callers with already-parsed inputs (e.g. an HTTP
    // server holding parsed payloads) actually pay; the prior
    // `Message::from_value(&data)` path measured the harness's serde_json
    // churn as part of the engine cost.
    let sample_payload_json = build_sample_payload();
    let sample_payload: Arc<OwnedDataValue> = Arc::new(OwnedDataValue::from(&sample_payload_json));

    // Warmup
    println!("Running warmup ({} messages)...", WARMUP_MESSAGES);
    let warmup_start = Instant::now();
    let warmup_handles: Vec<_> = (0..WARMUP_MESSAGES)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let payload = Arc::clone(&sample_payload);
            tokio::spawn(async move {
                let mut message = Message::new(payload);
                engine.process_message(&mut message).await.unwrap();
            })
        })
        .collect();
    join_all(warmup_handles).await;
    println!("Warmup completed in {:?}\n", warmup_start.elapsed());

    println!("Workload per message: 1 parse_json + 25 mappings + 12 validations = 38 ops");
    println!();
    println!(
        "Configuration | Messages | Concurrency | Throughput (msg/s) | Avg (μs) | P50 (μs) | P90 (μs) | P95 (μs) | P99 (μs) | P99.9 (μs)"
    );
    println!(
        "--------------|----------|-------------|-------------------|----------|----------|----------|----------|----------|------------"
    );

    let mut latency_stats = LatencyStats::new();
    let benchmark_start = Instant::now();
    let mut handles = Vec::with_capacity(TOTAL_MESSAGES);

    for _ in 0..TOTAL_MESSAGES {
        let engine = Arc::clone(&engine);
        let payload = Arc::clone(&sample_payload);
        handles.push(tokio::spawn(async move {
            let msg_start = Instant::now();
            let mut message = Message::new(payload);
            engine.process_message(&mut message).await.unwrap();
            msg_start.elapsed()
        }));
    }

    let latencies = join_all(handles).await;
    for d in latencies.into_iter().flatten() {
        latency_stats.add(d);
    }

    let total_time = benchmark_start.elapsed();
    let throughput = TOTAL_MESSAGES as f64 / total_time.as_secs_f64();
    let avg = latency_stats.average();
    let (p50, p90, p95, p99, p999) = latency_stats.percentiles();

    println!(
        "{:^13} | {:^8} | {:^11} | {:^17.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
        "Realistic",
        TOTAL_MESSAGES,
        "Unlimited",
        throughput,
        avg.as_micros(),
        p50.as_micros(),
        p90.as_micros(),
        p95.as_micros(),
        p99.as_micros(),
        p999.as_micros()
    );

    println!();
    println!(
        "Throughput per JSONLogic op: {:.0} ops/sec",
        throughput * 38.0
    );
    println!("\n✅ Benchmark complete!");

    Ok(())
}

fn build_workflow() -> Workflow {
    let json = r#"
    {
        "id": "iso20022_to_swiftmt103",
        "name": "ISO 20022 pacs.008 -> SwiftMT 103",
        "tasks": [
            {
                "id": "load_payload",
                "name": "Parse Payload",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
                }
            },
            {
                "id": "transform",
                "name": "Transform to MT103",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.MT103.20",     "logic": { "var": "data.input.GrpHdr.MsgId" } },
                            { "path": "data.MT103.23B",    "logic": "CRED" },
                            { "path": "data.MT103.32A.date",     "logic": { "var": "data.input.GrpHdr.CreDtTm" } },
                            { "path": "data.MT103.32A.currency", "logic": { "var": "data.input.CdtTrfTxInf.IntrBkSttlmAmt.Ccy" } },
                            { "path": "data.MT103.32A.amount",   "logic": { "var": "data.input.CdtTrfTxInf.IntrBkSttlmAmt.value" } },
                            { "path": "data.MT103.50K.name",     "logic": { "var": "data.input.CdtTrfTxInf.Dbtr.Nm" } },
                            { "path": "data.MT103.50K.address",
                              "logic": { "cat": [
                                  { "var": "data.input.CdtTrfTxInf.Dbtr.PstlAdr.StrtNm" }, " ",
                                  { "var": "data.input.CdtTrfTxInf.Dbtr.PstlAdr.TwnNm" }, ", ",
                                  { "var": "data.input.CdtTrfTxInf.Dbtr.PstlAdr.Ctry" }
                              ]}
                            },
                            { "path": "data.MT103.50K.account", "logic": { "var": "data.input.CdtTrfTxInf.DbtrAcct.Id.IBAN" } },
                            { "path": "data.MT103.52A.bic",     "logic": { "var": "data.input.CdtTrfTxInf.DbtrAgt.FinInstnId.BICFI" } },
                            { "path": "data.MT103.57A.bic",     "logic": { "var": "data.input.CdtTrfTxInf.CdtrAgt.FinInstnId.BICFI" } },
                            { "path": "data.MT103.59.name",     "logic": { "var": "data.input.CdtTrfTxInf.Cdtr.Nm" } },
                            { "path": "data.MT103.59.address",
                              "logic": { "cat": [
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.StrtNm" }, " ",
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.TwnNm" }, ", ",
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.Ctry" }
                              ]}
                            },
                            { "path": "data.MT103.59.account", "logic": { "var": "data.input.CdtTrfTxInf.CdtrAcct.Id.IBAN" } },
                            { "path": "data.MT103.70.remittance",
                              "logic": { "cat": [
                                  "/ROC/",
                                  { "var": "data.input.CdtTrfTxInf.RmtInf.Strd.RfrdDocInf.Nb" }
                              ]}
                            },
                            { "path": "data.MT103.71A.charges",
                              "logic": { "if": [
                                  { "==": [{ "var": "data.input.CdtTrfTxInf.ChrgBr" }, "SHAR"] }, "SHA",
                                  { "==": [{ "var": "data.input.CdtTrfTxInf.ChrgBr" }, "DEBT"] }, "OUR",
                                  "BEN"
                              ]}
                            },
                            { "path": "data.MT103.71F.amount",
                              "logic": { "if": [
                                  { ">": [{ "var": "data.input.CdtTrfTxInf.ChrgsInf.Amt.value" }, 0] },
                                  { "var": "data.input.CdtTrfTxInf.ChrgsInf.Amt.value" },
                                  null
                              ]}
                            },
                            { "path": "data.MT103.computed.totalWithCharges",
                              "logic": { "+": [
                                  { "var": "data.input.CdtTrfTxInf.IntrBkSttlmAmt.value" },
                                  { "var": "data.input.CdtTrfTxInf.ChrgsInf.Amt.value" }
                              ]}
                            },
                            { "path": "data.MT103.computed.feePercent",
                              "logic": { "*": [
                                  { "/": [
                                      { "var": "data.input.CdtTrfTxInf.ChrgsInf.Amt.value" },
                                      { "var": "data.input.CdtTrfTxInf.IntrBkSttlmAmt.value" }
                                  ]},
                                  100
                              ]}
                            },
                            { "path": "data.MT103.computed.isHighValue",
                              "logic": { ">=": [{ "var": "data.input.CdtTrfTxInf.IntrBkSttlmAmt.value" }, 100000] }
                            },
                            { "path": "data.MT103.computed.uetrPresent",
                              "logic": { "!!": { "var": "data.input.CdtTrfTxInf.PmtId.UETR" } }
                            },
                            { "path": "data.MT103.computed.creditorCountry",
                              "logic": { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.Ctry" }
                            },
                            { "path": "data.MT103.computed.debtorCountry",
                              "logic": { "var": "data.input.CdtTrfTxInf.Dbtr.PstlAdr.Ctry" }
                            },
                            { "path": "data.MT103.computed.crossBorder",
                              "logic": { "!=": [
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.Ctry" },
                                  { "var": "data.input.CdtTrfTxInf.Dbtr.PstlAdr.Ctry" }
                              ]}
                            },
                            { "path": "metadata.routing.channel",
                              "logic": { "if": [
                                  { "var": "data.MT103.computed.isHighValue" }, "rtgs",
                                  "ach"
                              ]}
                            },
                            { "path": "metadata.routing.priority",
                              "logic": { "if": [
                                  { "var": "data.MT103.computed.isHighValue" }, "high",
                                  { "var": "data.MT103.computed.crossBorder" }, "medium",
                                  "low"
                              ]}
                            },
                            { "path": "temp_data.uetr_canonical",
                              "logic": { "substr": [{ "var": "data.input.CdtTrfTxInf.PmtId.UETR" }, 0, 36] }
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate",
                "name": "Validate MT103",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            { "logic": { "!!": { "var": "data.MT103.20" } },           "message": "Field 20 (TRN) is required" },
                            { "logic": { "!!": { "var": "data.MT103.32A.currency" } }, "message": "Field 32A currency is required" },
                            { "logic": { ">": [{ "var": "data.MT103.32A.amount" }, 0] }, "message": "Field 32A amount must be positive" },
                            { "logic": { "!!": { "var": "data.MT103.50K.name" } },     "message": "Field 50K debtor name is required" },
                            { "logic": { "!!": { "var": "data.MT103.50K.account" } },  "message": "Field 50K debtor account is required" },
                            { "logic": { "!!": { "var": "data.MT103.59.name" } },      "message": "Field 59 creditor name is required" },
                            { "logic": { "!!": { "var": "data.MT103.59.account" } },   "message": "Field 59 creditor account is required" },
                            { "logic": { "!!": { "var": "data.MT103.52A.bic" } },      "message": "Field 52A debtor BIC is required" },
                            { "logic": { "!!": { "var": "data.MT103.57A.bic" } },      "message": "Field 57A creditor BIC is required" },
                            { "logic": { "var": "data.MT103.computed.uetrPresent" },   "message": "UETR is required for pacs.008 -> MT103" },
                            { "logic": { "<=": [{ "var": "data.MT103.computed.feePercent" }, 10] }, "message": "Fees must be <= 10% of principal" },
                            { "logic": { "in": [{ "var": "data.MT103.71A.charges" }, ["SHA", "OUR", "BEN"]] }, "message": "Field 71A charges must be SHA/OUR/BEN" }
                        ]
                    }
                }
            }
        ]
    }
    "#;
    Workflow::from_json(json).expect("workflow parses")
}

/// ISO 20022 pacs.008-shaped payload, ~80 fields across 4-5 levels.
fn build_sample_payload() -> serde_json::Value {
    json!({
        "GrpHdr": {
            "MsgId": "MSG-20260511-000001",
            "CreDtTm": "2026-05-11T12:34:56Z",
            "NbOfTxs": 1,
            "SttlmInf": {
                "SttlmMtd": "INDA",
                "ClrSys": { "Cd": "TGT2" }
            },
            "InstgAgt": { "FinInstnId": { "BICFI": "INSTGAG1XXX" } },
            "InstdAgt": { "FinInstnId": { "BICFI": "INSTDAG2XXX" } }
        },
        "CdtTrfTxInf": {
            "PmtId": {
                "InstrId": "INSTR-001",
                "EndToEndId": "E2E-001",
                "TxId": "TX-001",
                "UETR": "8e49e852-45a1-42f7-b120-18d232541285"
            },
            "PmtTpInf": {
                "InstrPrty": "NORM",
                "SvcLvl": { "Cd": "SEPA" },
                "LclInstrm": { "Cd": "CORE" },
                "CtgyPurp": { "Cd": "SALA" }
            },
            "IntrBkSttlmAmt": { "Ccy": "EUR", "value": 125000.50 },
            "IntrBkSttlmDt": "2026-05-12",
            "ChrgBr": "SHAR",
            "ChrgsInf": { "Amt": { "Ccy": "EUR", "value": 12.50 }, "Agt": { "FinInstnId": { "BICFI": "CHRGAG1XXX" } } },
            "Dbtr": {
                "Nm": "ACME Corporation GmbH",
                "PstlAdr": {
                    "StrtNm": "Hauptstrasse 100",
                    "BldgNb": "100",
                    "PstCd": "10115",
                    "TwnNm": "Berlin",
                    "Ctry": "DE"
                },
                "Id": { "OrgId": { "AnyBIC": "ACMEDEBEXXX" } }
            },
            "DbtrAcct": { "Id": { "IBAN": "DE89370400440532013000" }, "Ccy": "EUR" },
            "DbtrAgt": {
                "FinInstnId": {
                    "BICFI": "DEUTDEFFXXX",
                    "Nm": "Deutsche Bank AG",
                    "PstlAdr": { "TwnNm": "Frankfurt am Main", "Ctry": "DE" }
                }
            },
            "CdtrAgt": {
                "FinInstnId": {
                    "BICFI": "BNPAFRPPXXX",
                    "Nm": "BNP Paribas",
                    "PstlAdr": { "TwnNm": "Paris", "Ctry": "FR" }
                }
            },
            "Cdtr": {
                "Nm": "Beneficiaire SARL",
                "PstlAdr": {
                    "StrtNm": "Avenue des Champs-Elysees 50",
                    "BldgNb": "50",
                    "PstCd": "75008",
                    "TwnNm": "Paris",
                    "Ctry": "FR"
                },
                "Id": { "OrgId": { "AnyBIC": "BNFCFRPPXXX" } }
            },
            "CdtrAcct": { "Id": { "IBAN": "FR1420041010050500013M02606" }, "Ccy": "EUR" },
            "Purp": { "Cd": "SUPP" },
            "RmtInf": {
                "Strd": {
                    "RfrdDocInf": { "Tp": { "CdOrPrtry": { "Cd": "CINV" } }, "Nb": "INV-2026-04123" },
                    "RfrdDocAmt": { "DuePyblAmt": { "Ccy": "EUR", "value": 125000.50 } },
                    "AddtlRmtInf": "Payment for invoice INV-2026-04123 issued 2026-04-15"
                },
                "Ustrd": "Supplier payment April 2026"
            }
        }
    })
}
