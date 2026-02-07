#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BotState {
    IdleUSDT,
    BosPotential,
    BosConfirmed,
    Rebalancing,
    MMNormal,
    MMDefensive,
    Exiting,
}
