# Class diagram: core IR and types

```mermaid
classDiagram
  class FrontendModuleIR {
    +String module_name
    +FrontendSignature signature
    +FrontendClassification classification
    +ClockParamMeta[] clocks
    +RawStmt[] raw_statements
    +SourceSpan span
  }

  class FrontendSignature {
    +RawParam[] params
    +RawTypeRef? return_ty
  }

  class RawParam {
    +String name
    +RawTypeRef ty
    +String raw_text
    +SourceSpan span
  }

  class RawTypeRef {
    +String ty_text
    +SourceSpan span
  }

  class RawStmt {
    +usize order
    +RawStmtKind kind
    +String text
    +SourceSpan span
  }

  class RawStmtKind
  class ExprType

  class SourceSpan {
    +usize start_line
    +usize start_col
    +usize end_line
    +usize end_col
  }

  %% types
  class Logic
  class Bits
  class Clock

  %% memory
  class Memory {
    +size(): usize
    +read_port()<T>
    +write_port()<T>
  }

  %% relationships
  FrontendModuleIR --> FrontendSignature : has
  FrontendModuleIR --> RawStmt : contains
  FrontendModuleIR --> SourceSpan : spans
  FrontendSignature --> RawParam : params
  RawParam --> RawTypeRef : type
  RawStmt --> RawStmtKind : kind
  RawStmt --> SourceSpan : spans

  Bits --> Logic : uses
  Memory --> Clock : uses

  note right of FrontendModuleIR
    Focused view of captured module
  end note

```
