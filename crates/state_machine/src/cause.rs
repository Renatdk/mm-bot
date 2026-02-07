#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TransitionCause {
    // BOS lifecycle
    HtfBosUpDetected,
    BosConfirmed,
    BosFailed,
    PullbackDetected,

    // Rebalance
    RebalanceDone,
    RebalanceFailed,

    // MM behaviour
    LtfBosDown,
    LtfStructureRecovered,

    // Exit triggers
    HtfBosDown,
    BreakEvenHit,
    BreakEvenWithFeesHit,

    // Exit lifecycle
    ExitDone,
}
