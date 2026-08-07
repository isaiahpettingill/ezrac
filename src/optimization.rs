//! Compiler optimization levels and per-pass controls.

use crate::compat::prelude::*;

/// A compiler optimization pass that can be controlled independently.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OptimizationPass {
    ScalarSimplification,
    LocalPropagation,
    LoopInvariantCodeMotion,
    KnownBits,
    MemoryReadLoopInvariantCodeMotion,
    FunctionInlining,
    DeadCodeElimination,
    TailCalls,
    TailRecursion,
    IdempotentOperations,
    RedundantRegisterCopies,
    Mos6502Peepholes,
}

impl OptimizationPass {
    pub const ALL: [Self; 12] = [
        Self::ScalarSimplification,
        Self::LocalPropagation,
        Self::LoopInvariantCodeMotion,
        Self::KnownBits,
        Self::MemoryReadLoopInvariantCodeMotion,
        Self::FunctionInlining,
        Self::DeadCodeElimination,
        Self::TailCalls,
        Self::TailRecursion,
        Self::IdempotentOperations,
        Self::RedundantRegisterCopies,
        Self::Mos6502Peepholes,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::ScalarSimplification => "scalar-simplification",
            Self::LocalPropagation => "local-propagation",
            Self::LoopInvariantCodeMotion => "loop-invariant-code-motion",
            Self::KnownBits => "known-bits",
            Self::MemoryReadLoopInvariantCodeMotion => "memory-read-licm",
            Self::FunctionInlining => "function-inlining",
            Self::DeadCodeElimination => "dead-code-elimination",
            Self::TailCalls => "tail-calls",
            Self::TailRecursion => "tail-recursion",
            Self::IdempotentOperations => "idempotent-operations",
            Self::RedundantRegisterCopies => "redundant-register-copies",
            Self::Mos6502Peepholes => "mos6502-peepholes",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pass| pass.name() == name)
    }

    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|pass| pass.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    const fn enabled_at(self, level: u8) -> bool {
        match self {
            Self::DeadCodeElimination => true,
            Self::ScalarSimplification | Self::LocalPropagation | Self::KnownBits => level >= 1,
            Self::LoopInvariantCodeMotion
            | Self::MemoryReadLoopInvariantCodeMotion
            | Self::FunctionInlining
            | Self::TailCalls
            | Self::TailRecursion
            | Self::IdempotentOperations => level >= 2,
            Self::RedundantRegisterCopies => level >= 1,
            Self::Mos6502Peepholes => level >= 2,
        }
    }
}

/// Optimization level plus explicit pass overrides.
///
/// Explicit entries in `enable` and `disable` override the selected level.
/// If a pass appears in both lists, `disable` wins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimizationOptions {
    pub level: u8,
    pub enable: Vec<OptimizationPass>,
    pub disable: Vec<OptimizationPass>,
}

impl Default for OptimizationOptions {
    fn default() -> Self {
        Self {
            level: 2,
            enable: Vec::new(),
            disable: Vec::new(),
        }
    }
}

impl OptimizationOptions {
    pub fn new(level: u8) -> Result<Self, String> {
        if level > 3 {
            return Err(format!(
                "optimization level must be 0, 1, 2, or 3, got `{level}`"
            ));
        }
        Ok(Self {
            level,
            ..Self::default()
        })
    }

    pub fn is_enabled(&self, pass: OptimizationPass) -> bool {
        if self.disable.contains(&pass) {
            false
        } else if self.enable.contains(&pass) {
            true
        } else {
            pass.enabled_at(self.level)
        }
    }

    pub fn enable(&mut self, pass: OptimizationPass) {
        if !self.enable.contains(&pass) {
            self.enable.push(pass);
        }
        self.disable.retain(|candidate| *candidate != pass);
    }

    pub fn disable(&mut self, pass: OptimizationPass) {
        if !self.disable.contains(&pass) {
            self.disable.push(pass);
        }
        self.enable.retain(|candidate| *candidate != pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_overrides_select_passes() {
        let mut options = OptimizationOptions::new(0).unwrap();
        assert!(options.is_enabled(OptimizationPass::DeadCodeElimination));
        assert!(!options.is_enabled(OptimizationPass::ScalarSimplification));
        options.enable(OptimizationPass::ScalarSimplification);
        assert!(options.is_enabled(OptimizationPass::ScalarSimplification));
        options.disable(OptimizationPass::ScalarSimplification);
        assert!(!options.is_enabled(OptimizationPass::ScalarSimplification));
        options.disable(OptimizationPass::DeadCodeElimination);
        assert!(!options.is_enabled(OptimizationPass::DeadCodeElimination));

        let options = OptimizationOptions::new(1).unwrap();
        assert!(options.is_enabled(OptimizationPass::LocalPropagation));
        assert!(!options.is_enabled(OptimizationPass::FunctionInlining));

        let options = OptimizationOptions::new(2).unwrap();
        assert!(
            OptimizationPass::ALL
                .iter()
                .all(|pass| options.is_enabled(*pass))
        );
    }
}
