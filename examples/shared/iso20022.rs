//! The ISO 20022 pacs.008 → MT103-shaped workload: one `parse_json`, one `map`
//! with 25 mappings, one `validation` with 12 rules. `realistic_benchmark`
//! measures it alone. v4's port of v3's `examples/shared/iso20022.rs`, with the
//! same definition and payload.
//!
//! Not an example: pulled in with `#[path = "shared/iso20022.rs"] mod iso20022;`.

#![allow(dead_code)]

use dataflow_rs::Message;
use serde_json::{Value, json};

/// Checks a message the workload ran on: every rule passed and the document
/// is an object. Field tags such as `20` are spelled `#20` in the map paths,
/// because a bare numeric segment is an array index: unescaped, `data.MT103`
/// became a null-padded array, the keyed writes into it were dropped, and 8
/// of the 12 rules failed on every message.
pub fn check(message: &Message) {
    assert!(
        message.errors().is_empty(),
        "errors: {:?}",
        message.errors()
    );
    let data = Value::from(message.context().data());
    let mt103 = &data["MT103"];
    assert!(mt103.is_object(), "data.MT103 is an object, not {mt103}");
    assert_eq!(mt103["20"], json!("MSG-20260511-000001"));
    assert_eq!(mt103["32A"]["currency"], json!("EUR"));
}

pub fn workflow() -> Value {
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
                            { "path": "data.MT103.#20",     "logic": { "var": "data.input.GrpHdr.MsgId" } },
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
                            { "path": "data.MT103.#59.name",     "logic": { "var": "data.input.CdtTrfTxInf.Cdtr.Nm" } },
                            { "path": "data.MT103.#59.address",
                              "logic": { "cat": [
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.StrtNm" }, " ",
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.TwnNm" }, ", ",
                                  { "var": "data.input.CdtTrfTxInf.Cdtr.PstlAdr.Ctry" }
                              ]}
                            },
                            { "path": "data.MT103.#59.account", "logic": { "var": "data.input.CdtTrfTxInf.CdtrAcct.Id.IBAN" } },
                            { "path": "data.MT103.#70.remittance",
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
    serde_json::from_str(json).expect("workflow parses")
}

/// ISO 20022 pacs.008-shaped payload, ~80 fields across 4-5 levels.
pub fn payload() -> serde_json::Value {
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
