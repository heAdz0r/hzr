#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOperation {
    Replace,
    Patch,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    Text,
    Structured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteSemantics {
    pub operation: WriteOperation,
    pub class: WriteClass,
    pub mutating: bool,
    pub supports_dry_run: bool,
}

pub fn semantics_for(operation: WriteOperation) -> WriteSemantics {
    match operation {
        WriteOperation::Replace => WriteSemantics {
            operation,
            class: WriteClass::Text,
            mutating: true,
            supports_dry_run: true,
        },
        WriteOperation::Patch => WriteSemantics {
            operation,
            class: WriteClass::Text,
            mutating: true,
            supports_dry_run: true,
        },
        WriteOperation::Set => WriteSemantics {
            operation,
            class: WriteClass::Structured,
            mutating: true,
            supports_dry_run: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantics_are_mutating_and_dry_run_safe() {
        for op in [
            WriteOperation::Replace,
            WriteOperation::Patch,
            WriteOperation::Set,
        ] {
            let s = semantics_for(op);
            assert!(s.mutating);
            assert!(s.supports_dry_run);
        }
    }
}
