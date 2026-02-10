#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TrendCause {
    EntrySignal,
    ExitSignal,
    StopLossHit,
    ForceFlat,
}
