use crate::{diagnostic_set::DiagnosticEntry, Diagnostic, Operation};
use anyhow::{anyhow, Result};
use clock::ReplicaId;
use lsp::DiagnosticSeverity;
use rpc::proto;
use std::sync::Arc;
use text::*;

pub use proto::{Buffer, SelectionSet};

pub fn serialize_operation(operation: &Operation) -> proto::Operation {
    proto::Operation {
        variant: Some(match operation {
            Operation::Buffer(text::Operation::Edit(edit)) => {
                proto::operation::Variant::Edit(serialize_edit_operation(edit))
            }
            Operation::Buffer(text::Operation::Undo {
                undo,
                lamport_timestamp,
            }) => proto::operation::Variant::Undo(proto::operation::Undo {
                replica_id: undo.id.replica_id as u32,
                local_timestamp: undo.id.value,
                lamport_timestamp: lamport_timestamp.value,
                ranges: undo
                    .ranges
                    .iter()
                    .map(|r| proto::Range {
                        start: r.start.0 as u64,
                        end: r.end.0 as u64,
                    })
                    .collect(),
                counts: undo
                    .counts
                    .iter()
                    .map(|(edit_id, count)| proto::operation::UndoCount {
                        replica_id: edit_id.replica_id as u32,
                        local_timestamp: edit_id.value,
                        count: *count,
                    })
                    .collect(),
                version: From::from(&undo.version),
            }),
            Operation::UpdateSelections {
                replica_id,
                selections,
                lamport_timestamp,
            } => proto::operation::Variant::UpdateSelections(proto::operation::UpdateSelections {
                replica_id: *replica_id as u32,
                lamport_timestamp: lamport_timestamp.value,
                selections: serialize_selections(selections),
            }),
            Operation::RemoveSelections {
                replica_id,
                lamport_timestamp,
            } => proto::operation::Variant::RemoveSelections(proto::operation::RemoveSelections {
                replica_id: *replica_id as u32,
                lamport_timestamp: lamport_timestamp.value,
            }),
            Operation::UpdateDiagnostics {
                provider_name,
                diagnostics,
                lamport_timestamp,
            } => proto::operation::Variant::UpdateDiagnosticSet(proto::UpdateDiagnosticSet {
                replica_id: lamport_timestamp.replica_id as u32,
                lamport_timestamp: lamport_timestamp.value,
                diagnostic_set: Some(serialize_diagnostic_set(
                    provider_name.clone(),
                    diagnostics.iter(),
                )),
            }),
        }),
    }
}

pub fn serialize_edit_operation(operation: &EditOperation) -> proto::operation::Edit {
    let ranges = operation
        .ranges
        .iter()
        .map(|range| proto::Range {
            start: range.start.0 as u64,
            end: range.end.0 as u64,
        })
        .collect();
    proto::operation::Edit {
        replica_id: operation.timestamp.replica_id as u32,
        local_timestamp: operation.timestamp.local,
        lamport_timestamp: operation.timestamp.lamport,
        version: From::from(&operation.version),
        ranges,
        new_text: operation.new_text.clone(),
    }
}

pub fn serialize_selections(selections: &Arc<[Selection<Anchor>]>) -> Vec<proto::Selection> {
    selections
        .iter()
        .map(|selection| proto::Selection {
            id: selection.id as u64,
            start: Some(serialize_anchor(&selection.start)),
            end: Some(serialize_anchor(&selection.end)),
            reversed: selection.reversed,
        })
        .collect()
}

pub fn serialize_diagnostic_set<'a>(
    provider_name: String,
    diagnostics: impl IntoIterator<Item = &'a DiagnosticEntry<Anchor>>,
) -> proto::DiagnosticSet {
    proto::DiagnosticSet {
        provider_name,
        diagnostics: diagnostics
            .into_iter()
            .map(|entry| proto::Diagnostic {
                start: Some(serialize_anchor(&entry.range.start)),
                end: Some(serialize_anchor(&entry.range.end)),
                message: entry.diagnostic.message.clone(),
                severity: match entry.diagnostic.severity {
                    DiagnosticSeverity::ERROR => proto::diagnostic::Severity::Error,
                    DiagnosticSeverity::WARNING => proto::diagnostic::Severity::Warning,
                    DiagnosticSeverity::INFORMATION => proto::diagnostic::Severity::Information,
                    DiagnosticSeverity::HINT => proto::diagnostic::Severity::Hint,
                    _ => proto::diagnostic::Severity::None,
                } as i32,
                group_id: entry.diagnostic.group_id as u64,
                is_primary: entry.diagnostic.is_primary,
                is_valid: entry.diagnostic.is_valid,
                code: entry.diagnostic.code.clone(),
                is_disk_based: entry.diagnostic.is_disk_based,
            })
            .collect(),
    }
}

fn serialize_anchor(anchor: &Anchor) -> proto::Anchor {
    proto::Anchor {
        replica_id: anchor.timestamp.replica_id as u32,
        local_timestamp: anchor.timestamp.value,
        offset: anchor.offset as u64,
        bias: match anchor.bias {
            Bias::Left => proto::Bias::Left as i32,
            Bias::Right => proto::Bias::Right as i32,
        },
    }
}

pub fn deserialize_operation(message: proto::Operation) -> Result<Operation> {
    Ok(
        match message
            .variant
            .ok_or_else(|| anyhow!("missing operation variant"))?
        {
            proto::operation::Variant::Edit(edit) => {
                Operation::Buffer(text::Operation::Edit(deserialize_edit_operation(edit)))
            }
            proto::operation::Variant::Undo(undo) => Operation::Buffer(text::Operation::Undo {
                lamport_timestamp: clock::Lamport {
                    replica_id: undo.replica_id as ReplicaId,
                    value: undo.lamport_timestamp,
                },
                undo: UndoOperation {
                    id: clock::Local {
                        replica_id: undo.replica_id as ReplicaId,
                        value: undo.local_timestamp,
                    },
                    counts: undo
                        .counts
                        .into_iter()
                        .map(|c| {
                            (
                                clock::Local {
                                    replica_id: c.replica_id as ReplicaId,
                                    value: c.local_timestamp,
                                },
                                c.count,
                            )
                        })
                        .collect(),
                    ranges: undo
                        .ranges
                        .into_iter()
                        .map(|r| FullOffset(r.start as usize)..FullOffset(r.end as usize))
                        .collect(),
                    version: undo.version.into(),
                },
            }),
            proto::operation::Variant::UpdateSelections(message) => {
                let selections = message
                    .selections
                    .into_iter()
                    .filter_map(|selection| {
                        Some(Selection {
                            id: selection.id as usize,
                            start: deserialize_anchor(selection.start?)?,
                            end: deserialize_anchor(selection.end?)?,
                            reversed: selection.reversed,
                            goal: SelectionGoal::None,
                        })
                    })
                    .collect::<Vec<_>>();

                Operation::UpdateSelections {
                    replica_id: message.replica_id as ReplicaId,
                    lamport_timestamp: clock::Lamport {
                        replica_id: message.replica_id as ReplicaId,
                        value: message.lamport_timestamp,
                    },
                    selections: Arc::from(selections),
                }
            }
            proto::operation::Variant::RemoveSelections(message) => Operation::RemoveSelections {
                replica_id: message.replica_id as ReplicaId,
                lamport_timestamp: clock::Lamport {
                    replica_id: message.replica_id as ReplicaId,
                    value: message.lamport_timestamp,
                },
            },
            proto::operation::Variant::UpdateDiagnosticSet(message) => {
                let (provider_name, diagnostics) = deserialize_diagnostic_set(
                    message
                        .diagnostic_set
                        .ok_or_else(|| anyhow!("missing diagnostic set"))?,
                );
                Operation::UpdateDiagnostics {
                    provider_name,
                    diagnostics,
                    lamport_timestamp: clock::Lamport {
                        replica_id: message.replica_id as ReplicaId,
                        value: message.lamport_timestamp,
                    },
                }
            }
        },
    )
}

pub fn deserialize_edit_operation(edit: proto::operation::Edit) -> EditOperation {
    let ranges = edit
        .ranges
        .into_iter()
        .map(|range| FullOffset(range.start as usize)..FullOffset(range.end as usize))
        .collect();
    EditOperation {
        timestamp: InsertionTimestamp {
            replica_id: edit.replica_id as ReplicaId,
            local: edit.local_timestamp,
            lamport: edit.lamport_timestamp,
        },
        version: edit.version.into(),
        ranges,
        new_text: edit.new_text,
    }
}

pub fn deserialize_selections(selections: Vec<proto::Selection>) -> Arc<[Selection<Anchor>]> {
    Arc::from(
        selections
            .into_iter()
            .filter_map(|selection| {
                Some(Selection {
                    id: selection.id as usize,
                    start: deserialize_anchor(selection.start?)?,
                    end: deserialize_anchor(selection.end?)?,
                    reversed: selection.reversed,
                    goal: SelectionGoal::None,
                })
            })
            .collect::<Vec<_>>(),
    )
}

pub fn deserialize_diagnostic_set(
    message: proto::DiagnosticSet,
) -> (String, Arc<[DiagnosticEntry<Anchor>]>) {
    (
        message.provider_name,
        message
            .diagnostics
            .into_iter()
            .filter_map(|diagnostic| {
                Some(DiagnosticEntry {
                    range: deserialize_anchor(diagnostic.start?)?
                        ..deserialize_anchor(diagnostic.end?)?,
                    diagnostic: Diagnostic {
                        severity: match proto::diagnostic::Severity::from_i32(diagnostic.severity)?
                        {
                            proto::diagnostic::Severity::Error => DiagnosticSeverity::ERROR,
                            proto::diagnostic::Severity::Warning => DiagnosticSeverity::WARNING,
                            proto::diagnostic::Severity::Information => {
                                DiagnosticSeverity::INFORMATION
                            }
                            proto::diagnostic::Severity::Hint => DiagnosticSeverity::HINT,
                            proto::diagnostic::Severity::None => return None,
                        },
                        message: diagnostic.message,
                        group_id: diagnostic.group_id as usize,
                        code: diagnostic.code,
                        is_valid: diagnostic.is_valid,
                        is_primary: diagnostic.is_primary,
                        is_disk_based: diagnostic.is_disk_based,
                    },
                })
            })
            .collect(),
    )
}

fn deserialize_anchor(anchor: proto::Anchor) -> Option<Anchor> {
    Some(Anchor {
        timestamp: clock::Local {
            replica_id: anchor.replica_id as ReplicaId,
            value: anchor.local_timestamp,
        },
        offset: anchor.offset as usize,
        bias: match proto::Bias::from_i32(anchor.bias)? {
            proto::Bias::Left => Bias::Left,
            proto::Bias::Right => Bias::Right,
        },
    })
}
