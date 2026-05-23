```mermaid
classDiagram
    class copper-codegen_src_VerilogGenerator {
        +generate(ir:&ModuleIR) String
        +generate_statement(stmt:&Statement, indent:usize) String
        +generate_expression(expr:&Expression) String
        +validate_identifier(name:&str) Result~(), String~
    }
    class copper-codegen_src_LowerCtx {
        -hardware_fns: &'a std::collections::HashSet~String~
        -registry: &'a ModuleRegistry
        -submodules: Vec~copper-core_src_CHIRSubmoduleInst~
        -inst_counters: std::collections::HashMap~String, usize~
        -clock_name: String
    }
    class copper-codegen_src_HasSpan {
        +span() &SourceSpan
    }
    class copper-codegen_src_LowerError {
        +fmt(f:&mut std::fmt::Formatter) std::fmt::Result
    }
    class copper-codegen_src_IRBuilder {
        +from_ast(_design_fn:&ItemFn, ports:Vec~copper-core_src_Port~) Result~copper-core_src_ModuleIR, copper-codegen_src_LowerError~
    }
    class copper-core_src_ModuleIR {
        +name: String
        +ports: Vec~copper-core_src_Port~
        +statements: Vec~Statement~
        +submodules: Vec~copper-core_src_ModuleIR~
    }
    class copper-core_src_Assignment {
        +target: copper-core_src_Signal
        +value: Expression
    }
    class copper-core_src_CaseArm {
        +pattern: Pattern
        +body: Vec~Statement~
    }
    class copper-core_src_CaseExprArm {
        +pattern: Pattern
        +value: Box~Expression~
    }
    class copper-core_src_Signal {
        +name: String
        +width: usize
    }
    class copper-core_src_Bit {
        +from_bool(b:bool) Self
        +as_bool() bool
        +is_valid() bool
        +from(b:bool) Self
        +from(l:Logic) Self
        +not() Self::Output
        +bitand(rhs:Self) Self::Output
        +bitor(rhs:Self) Self::Output
        +bitxor(rhs:Self) Self::Output
        +fmt(f:&mut fmt::Formatter~'_~) fmt::Result
    }
    class copper-core_src_Bits {
        -bits: [Logic; N]
    }
    class copper-core_src_Default {
        +default() Self
        +default() Self
    }
    class copper-core_src_HasUnknown {
        +unknown() Self
        +unknown() Self
        +unknown() Self
        +unknown() Self
        +unknown() Self
        +unknown() Self
    }
    class copper-core_src_ClockState {
        -cycle: u64
        -wakers: Vec~Waker~
        -listeners: Vec~std::sync::Weak<dyn ClockEdgeListener>~
    }
    class copper-core_src_Clock {
        -state: Arc~Mutex<ClockState>~
        -_domain: PhantomData~Domain~
    }
    class copper-core_src_Clone {
        +clone() Self
    }
    class copper-core_src_ClockTick {
        -state: Arc~Mutex<ClockState>~
        -target_cycle: u64
        -_domain: PhantomData~Domain~
    }
    class copper-core_src_FrontendModuleIR {
        +module_name: String
        +signature: copper-core_src_FrontendSignature
        +classification: FrontendClassification
        +clocks: Vec~copper-core_src_ClockParamMeta~
        +raw_statements: Vec~copper-core_src_RawStmt~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_FrontendSignature {
        +params: Vec~copper-core_src_RawParam~
        +return_ty: Option~copper-core_src_RawTypeRef~
    }
    class copper-core_src_RawParam {
        +name: String
        +ty: copper-core_src_RawTypeRef
        +raw_text: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_RawTypeRef {
        +ty_text: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ClockParamMeta {
        +param_idx: usize
        +param_name: String
        +clock_ty: String
        +domain_hint: Option~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_LocalStmt {
        +is_mut: bool
        +ty: Option~copper-core_src_RawTypeRef~
        +name: String
        +init: Option~ExprType~
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemConst {
        +name: String
        +ty: copper-core_src_RawTypeRef
        +value_text: String
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemEnum {
        +name: String
        +variants: Vec~copper-core_src_EnumVariant~
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_EnumVariant {
        +name: String
        +discriminant: Option~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemStruct {
        +name: String
        +fields: Vec~copper-core_src_StructField~
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_StructField {
        +name: String
        +ty: copper-core_src_RawTypeRef
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemType {
        +name: String
        +target_ty: copper-core_src_RawTypeRef
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemMacro {
        +name: String
        +body_text: String
        +attrs: Vec~String~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ItemOther {
        +text: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprStmt {
        +expr: ExprType
        +has_semi: bool
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprArray {
        +elements: Vec~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprAssign {
        +left: Box~ExprType~
        +right: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprAsync {
        +is_move: bool
        +block: Vec~copper-core_src_RawStmt~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprAwait {
        +base: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprBinary {
        +left: Box~ExprType~
        +op: String
        +right: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprCall {
        +func: Box~ExprType~
        +args: Vec~ExprType~
        +is_hardware_module: bool
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprCast {
        +expr: Box~ExprType~
        +target_ty: copper-core_src_RawTypeRef
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprField {
        +base: Box~ExprType~
        +member: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprIf {
        +condition: Box~ExprType~
        +then_block: Vec~copper-core_src_RawStmt~
        +else_branch: Option~Box<ExprType>~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprLet {
        +pattern_text: String
        +expr: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprLit {
        +text: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprLoop {
        +body: Vec~copper-core_src_RawStmt~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprMatchArm {
        +pattern_text: String
        +guard: Option~Box<ExprType>~
        +body: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprMatch {
        +scrutinee: Box~ExprType~
        +arms: Vec~copper-core_src_ExprMatchArm~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprMethodCall {
        +receiver: Box~ExprType~
        +method: String
        +args: Vec~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprRange {
        +start: Option~Box<ExprType>~
        +end: Option~Box<ExprType>~
        +inclusive: bool
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprReference {
        +is_mut: bool
        +expr: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprRepeat {
        +expr: Box~ExprType~
        +len: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprReturn {
        +value: Option~Box<ExprType>~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprUnary {
        +op: String
        +expr: Box~ExprType~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprWhile {
        +condition: Box~ExprType~
        +body: Vec~copper-core_src_RawStmt~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_ExprYield {
        +value: Option~Box<ExprType>~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_RawStmt {
        +order: usize
        +kind: RawStmtKind
        +text: String
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_SourceSpan {
        +start_line: usize
        +start_col: usize
        +end_line: usize
        +end_col: usize
    }
    class copper-core_src_Wire {
        -name: String
        -value: [Logic; N]
        -dir: Direction
    }
    class copper-core_src_Register {
        -name: String
        -value: [Logic; N]
        -dir: Direction
    }
    class copper-core_src_Port {
        +name: String
        +width: usize
        +direction: Direction
    }
    class copper-core_src_FunctionAst {
        +name: String
        +ast: String
    }
    class copper-core_src_Memory {
        -shared: Arc~MemoryShared<T, R, W, READ_LAT, WRITE_LAT>~
        -_clock: copper-core_src_Clock~D~
        -read_mode: ReadMode
    }
    class copper-core_src_MemoryInner {
        -data: Vec~T~
        -write_pipeline: [[Option~(usize, T)~
        -read_pipeline: [[Option~T~
        -pending_read_addr: [Option~usize~
    }
    class copper-core_src_MemoryShared {
        -inner: Mutex~MemoryInner<T, R, W, READ_LAT, WRITE_LAT>~
        -write_first_mode: AtomicBool
    }
    class copper-core_src_ClockEdgeListener {
        +on_posedge() void
    }
    class copper-core_src_ReadPort {
        -mem: &'a Memory~T, R, W, D, READ_LAT, WRITE_LAT~
    }
    class copper-core_src_WritePort {
        -mem: &'a Memory~T, R, W, D, READ_LAT, WRITE_LAT~
    }
    class copper-core_src_SHIRModule {
        +name: String
        +ports: Vec~copper-core_src_SHIRPort~
        +body: SHIRBody
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_SHIRPort {
        +name: String
        +direction: SHIRPortDir
        +kind: SHIRPortKind
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_SHIRCombBody {
        +submodules: Vec~copper-core_src_SHIRSubmoduleInst~
        +wires: Vec~copper-core_src_SHIRWire~
        +output_expr: SHIRExpr
    }
    class copper-core_src_SHIRWire {
        +name: String
        +ty: CHIRType
        +value: SHIRExpr
    }
    class copper-core_src_SHIRSeqBody {
        +clock: String
        +registers: Vec~copper-core_src_SHIRReg~
        +submodules: Vec~copper-core_src_SHIRSubmoduleInst~
        +phases: Vec~copper-core_src_SHIRPhase~
        +output_drive: Option~SHIROutputDrive~
    }
    class copper-core_src_SHIRReg {
        +name: String
        +ty: CHIRType
        +init: Option~copper-core_src_SHIRLit~
    }
    class copper-core_src_SHIRSubmoduleInst {
        +inst_name: String
        +module_name: String
        +inputs: Vec~(String, SHIRExpr)~
        +output_wire: String
        +output_ty: CHIRType
    }
    class copper-core_src_SHIRPhase {
        +phase_idx: usize
        +pre_edge: Vec~SHIRStmt~
        +post_edge: Vec~copper-core_src_SHIRRegUpdate~
    }
    class copper-core_src_SHIRRegUpdate {
        +target: String
        +next_value: SHIRExpr
    }
    class copper-core_src_SHIRPhaseOutputArm {
        +phase_idx: usize
        +value: SHIRExpr
    }
    class copper-core_src_SHIRMatchArm {
        +patterns: Vec~SHIRPattern~
        +guard: Option~SHIRExpr~
        +stmts: Vec~SHIRStmt~
    }
    class copper-core_src_SHIRCaseArm {
        +pattern: SHIRPattern
        +guard: Option~SHIRExpr~
        +value: SHIRExpr
    }
    class copper-core_src_SHIRLit {
        +ty: CHIRType
        +value: u128
    }
    class copper-core_src_SHIRLowerError {
        +fmt(f:&mut std::fmt::Formatter~'_~) std::fmt::Result
    }
    class copper-core_src_CHIRModule {
        +name: String
        +ports: Vec~copper-core_src_CHIRPort~
        +body: CHIRBody
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRPort {
        +name: String
        +direction: CHIRPortDir
        +kind: CHIRPortKind
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRCombBody {
        +submodules: Vec~copper-core_src_CHIRSubmoduleInst~
        +wires: Vec~copper-core_src_CHIRWireDecl~
        +output: CHIRExpr
    }
    class copper-core_src_CHIRWireDecl {
        +name: String
        +ty: CHIRType
        +value: CHIRExpr
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRSeqBody {
        +clock: String
        +registers: Vec~copper-core_src_CHIRRegDecl~
        +submodules: Vec~copper-core_src_CHIRSubmoduleInst~
        +loop_body: Vec~CHIRStmt~
    }
    class copper-core_src_CHIRRegDecl {
        +name: String
        +ty: CHIRType
        +init: Option~copper-core_src_CHIRLit~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRSubmoduleInst {
        +inst_name: String
        +module_name: String
        +inputs: Vec~(String, CHIRExpr)~
        +output_wire: String
        +output_ty: CHIRType
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRMatchArm {
        +patterns: Vec~CHIRPattern~
        +guard: Option~CHIRExpr~
        +body: Vec~CHIRStmt~
        +span: copper-core_src_SourceSpan
    }
    class copper-core_src_CHIRCaseArm {
        +pattern: CHIRPattern
        +guard: Option~CHIRExpr~
        +value: CHIRExpr
    }
    class copper-core_src_CHIRLit {
        +ty: CHIRType
        +value: u128
    }
    class copper-core_src_CHIRLowerError {
        +fmt(f:&mut std::fmt::Formatter~'_~) std::fmt::Result
    }
    class tests_FifoClk {
    }
    class tests_ClockDomain {
    }
    class tests_FifoStep {
        -wr_en: bool
        -rd_en: bool
        -din: u8
    }
    class tests_FifoOutputs {
        -dout: u8
        -empty: bool
        -full: bool
        -valid: bool
        -count: u8
    }
    class tests_MemoryBackedFifo {
        -clock: copper-core_src_Clock~tests_FifoClk~
        -mem: copper-core_src_Memory~u8, 1, 1, tests_FifoClk~
        -read_ptr: usize
        -write_ptr: usize
        -count: usize
        +new() Self
        +cycle(wr_en:bool, rd_en:bool, din:u8) tests_FifoOutputs
    }
    class tests_MainClk {
    }
    class tests_ClockDomain {
    }
    class tests_TestClk {
    }
    class tests_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_memory_MainClk {
    }
    class examples_memory_ClockDomain {
    }
    class examples_cpu_MainClk {
    }
    class examples_cpu_ClockDomain {
    }
    class examples_cpu_RType {
        -opcode: examples_cpu_Opcode
        -rd: RegIndex
        -funct3: Funct3
        -rs1: RegIndex
        -rs2: RegIndex
        -funct7: Funct7
    }
    class examples_cpu_IType {
        -opcode: examples_cpu_Opcode
        -rd: RegIndex
        -funct3: Funct3
        -rs1: RegIndex
        -imm12: Immediate12
    }
    class examples_cpu_SType {
        -opcode: examples_cpu_Opcode
        -imm5: Immediate5
        -funct3: Funct3
        -rs1: RegIndex
        -rs2: RegIndex
        -imm7: Immediate7
    }
    class examples_cpu_BType {
        -opcode: examples_cpu_Opcode
        -imm5: Immediate5
        -funct3: Funct3
        -rs1: RegIndex
        -rs2: RegIndex
        -imm7: Immediate7
    }
    class examples_cpu_UType {
        -opcode: examples_cpu_Opcode
        -rd: RegIndex
        -imm20: Immediate20
    }
    class examples_cpu_JType {
        -opcode: examples_cpu_Opcode
        -rd: RegIndex
        -imm20: Immediate20
    }
    class examples_cpu_MainClk {
    }
    class examples_cpu_ClockDomain {
    }
    class examples_cpu_MainClk {
    }
    class examples_cpu_ClockDomain {
    }
    class examples_cpu_Opcode {
        +from_u32(op:u32) Option~Self~
    }
    class examples_cpu_BranchCond {
        +from_f3(f3:u32) Option~Self~
    }
    class examples_cpu_InstrDecoded {
        +opcode: examples_cpu_Opcode
        +rd: usize
        +rs1: usize
        +rs2: usize
        +f3: u32
        +f7: u32
        +imm_i: i32
        +imm_s: i32
        +imm_b: i32
        +imm_j: i32
        +imm_u: u32
    }
    class examples_cpu_AluOutput {
        +result: u32
        +overflow: bool
        +zero: bool
        +negative: bool
    }
    class examples_cpu_MainClk {
    }
    class examples_cpu_CpuState {
        -pc: u32
        -regs: Vec~u32~
        -imem: Arc~Mutex<Vec<u32>>~
        -dmem: Arc~Mutex<Vec<u32>>~
        +new(imem:Arc~Mutex<Vec<u32>>~, dmem:Arc~Mutex<Vec<u32>>~) Self
        +get_reg(idx:u32) u32
        +set_reg(idx:u32, val:u32) void
        +step() void
    }
    class examples_cpu_MainClk {
    }
    class examples_cpu_ClockDomain {
    }
    class examples_cpu_Opcode {
        +from_u32(op:u32) Option~Self~
    }
    class examples_cpu_BranchCond {
        +from_f3(f3:u32) Option~Self~
    }
    class examples_cpu_InstrDecoded {
        +opcode: examples_cpu_Opcode
        +rd: usize
        +rs1: usize
        +rs2: usize
        +f3: u32
        +f7: u32
        +imm_i: i32
        +imm_s: i32
        +imm_b: i32
        +imm_j: i32
        +imm_u: u32
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_timing_MainClk {
    }
    class examples_timing_ClockDomain {
    }
    class examples_combinational_MainClk {
    }
    class examples_combinational_ClockDomain {
    }
    class examples_combinational_MainClk {
    }
    class examples_combinational_ClockDomain {
    }
    class examples_combinational_MainClk {
    }
    class examples_combinational_ClockDomain {
    }
    class examples_combinational_MainClk {
    }
    class examples_combinational_ClockDomain {
    }
    class examples_verification_ClockDomain {
    }
    class examples_verification_MainClk {
    }
    class examples_verification_ClockDomain {
    }
    class examples_verification_ClockDomain {
    }
    class examples_helpers_BinaryTestError {
        +from(err:std::io::Error) Self
        +fmt(f:&mut std::fmt::Formatter~'_~) std::fmt::Result
    }
    class examples_helpers_RV32IProgram {
        +instructions: Vec~u32~
        +entry_point: u32
        +source: String
        +new(instructions:Vec~u32~) Self
        +from_elf(path:P) ~P: AsRef<Path>~
        +from_raw(path:P) ~P: AsRef<Path>~
        +disassemble_summary() String
    }
    class examples_helpers_BinaryTestRunner {
        -program: examples_helpers_RV32IProgram
        +new(program:examples_helpers_RV32IProgram) Self
        +load_elf(path:P) ~P: AsRef<Path>~
        +load_raw(path:P) ~P: AsRef<Path>~
        +program() &RV32IProgram
        +program_mut() &mut RV32IProgram
        +print_summary() void
    }
    class examples_helpers_CpuTestConfig {
        +max_cycles: usize
        +verbose: bool
        +with_max_cycles(max_cycles:usize) Self
        +verbose() Self
    }
    class examples_helpers_Default {
        +default() Self
    }
    class examples_MainClk {
    }
    class examples_ClockDomain {
    }
    class examples_MainClk {
    }
    class examples_ClockDomain {
    }
    class examples_MainClk {
    }
    class examples_ClockDomain {
    }
    class examples_MainClk {
    }
    class examples_ClockDomain {
    }
    class copper-sim_src_SimulationTrace {
        +cycles: Vec~copper-sim_src_CycleData~
        +new() Self
        +from_cycles(cycles:Vec~copper-sim_src_CycleData~) Self
        +add_cycle(cycle:usize, inputs:Vec~(String, Vec<Logic>)~, outputs:Vec~(String, Vec<Logic>)~) void
        +export_vcd(path:&str, module_name:&str) Result~(), String~
    }
    class copper-sim_src_CycleData {
        +cycle: usize
        +inputs: Vec~(String, Vec<Logic>)~
        +outputs: Vec~(String, Vec<Logic>)~
    }
    class copper-sim_src_EmitTargetGuard {
        -previous: Option~Arc<dyn Any + Send + Sync>~
    }
    class copper-sim_src_Drop {
        +drop() void
    }
    class copper-sim_src_DeltaYield {
        -yielded: bool
    }
    class copper-sim_src_Future {
        +poll(_cx:&mut Context~'_~) Poll~()~
    }
    class copper-sim_src_Simulator {
        -module: M
        -cycle: u64
        -waveforms: HashMap~String, Vec<u64>~
    }
    class copper-sim_src_HardwareExecutor {
        -tasks: Vec~copper-sim_src_TaskEntry~
        -cycle: u64
        -modules: HashMap~String, copper-sim_src_ModuleInfo~
        +new() Self
        +spawn(future:F) ~F, T~
        +spawn_function_typed(initial_output:T, future:F) ~T, F~
        +spawn_function_typed_with_unknown(initial_output:T, future:F) ~T, F~
        +spawn_into_with_unknown(output:Arc~Mutex<T>~, future:F) ~T, F~
        +spawn_child(child_name:&str, parent_name:&str, future:F) ~F, T~
        +spawn_child_function_typed(child_name:&str, parent_name:&str, initial_output:T, future:F) ~T, F~
        +spawn_child_function_typed_with_unknown(child_name:&str, parent_name:&str, initial_output:T, future:F) ~T, F~
        +module_info(module_name:&str) Option~&ModuleInfo~
        +module_infos() &HashMap~String, copper-sim_src_ModuleInfo~
        +poll_tasks() void
        +advance(clk:&mut Clock~Domain~) ~Domain: ClockDomain~
        +tick_clock(clk:&mut Clock~Domain~) ~Domain: ClockDomain~
        +cycle() u64
        +ensure_module(module_name:&str) void
    }
    class copper-sim_src_TaskEntry {
        -future: Pin~Box<dyn Future<Output = ()>>~
        -emit_target: Option~Arc<dyn Any + Send + Sync>~
        -set_unknown: Option~Box<dyn Fn() + Send + Sync>~
        -consecutive_dirty: usize
    }
    class copper-sim_src_ModuleInfo {
        +name: String
        +parent: Option~String~
        +children: Vec~String~
    }
    class copper-sim_src_Default {
        +default() Self
    }
    class copper-sim_src_HardwareTest {
        -name: String
        -verilog_path: Option~String~
        -waveform_path: Option~String~
        -phased_waveform_path: Option~String~
        -verilator_waveform_path: Option~String~
        -actual_trace: copper-sim_src_SimulationTrace
        -phase_data: Vec~copper-sim_src_PhasedCycleData~
        +new(name:&str) Self
        +with_verilog(path:&str) Self
        +with_waveform(path:&str) Self
        +with_phased_waveform(path:&str) Self
        +with_verilator_waveform(path:&str) Self
        +record_cycle(cycle:usize, inputs:&[(&str, &[Logic])], outputs:&[(&str, &[Logic])]) void
        +record_cycle_phased(cycle:usize, inputs:&[(&str, &[Logic])], pre_outputs:&[(&str, &[Logic])], post_outputs:&[(&str, &[Logic])]) void
        +finish() copper-sim_src_TestResult
        +finish_with_expected(expected:&SimulationTrace) copper-sim_src_TestResult
        +finish_internal(expected:Option~&SimulationTrace~) copper-sim_src_TestResult
    }
    class copper-sim_src_PhasedCycleData {
        -cycle: usize
        -pre_signals: Vec~(String, Vec<Logic>)~
        -post_signals: Vec~(String, Vec<Logic>)~
    }
    class copper-sim_src_TestResult {
        +name: String
        +trace_match: Option~bool~
        +verilator_ok: Option~bool~
        +waveform_path: Option~String~
        +phased_waveform_path: Option~String~
        +verilator_waveform_path: Option~String~
        +errors: Vec~String~
        +passed() bool
        +print_summary() void
        +assert_passed() void
    }
    class copper-macros_tests_ui_fail_MainClk {
    }
    class copper-macros_tests_ui_fail_ClockDomain {
    }
    class copper-macros_tests_ui_fail_MainClk {
    }
    class copper-macros_tests_ui_fail_ClockDomain {
    }
    class copper-macros_tests_ui_fail_MainClk {
    }
    class copper-macros_tests_ui_fail_ClockDomain {
    }
    class copper-macros_tests_ui_fail_MainClk {
    }
    class copper-macros_tests_ui_fail_ClockDomain {
    }
    class copper-macros_tests_ui_pass_MainClk {
    }
    class copper-macros_tests_ui_pass_ClockDomain {
    }
    class copper-macros_tests_ui_pass_MainClk {
    }
    class copper-macros_tests_ui_pass_ClockDomain {
    }
    class copper-macros_tests_ui_pass_MainClk {
    }
    class copper-macros_tests_ui_pass_ClockDomain {
    }
namespace tests {
    class tests_copper-core_src_TestClk {
    }
    class tests_copper-core_src_ClockDomain {
    }
    class tests_copper-sim_src_TestClk {
    }
    class tests_copper-sim_src_ClockDomain {
    }
}
namespace rv32i_types {
    class rv32i_types_examples_cpu_Opcode {
        +from_u32(op:u32) Option~Self~
    }
    class rv32i_types_examples_cpu_InstrDecoded {
        +opcode: rv32i_types_examples_cpu_Opcode
        +rd: usize
        +rs1: usize
        +rs2: usize
        +f3: u32
        +f7: u32
        +imm_i: i32
        +imm_s: i32
        +imm_b: i32
        +imm_j: i32
        +imm_u: u32
    }
    class rv32i_types_examples_cpu_BranchCond {
        +from_f3(f3:u32) Option~Self~
    }
    class rv32i_types_examples_cpu_AluOutput {
        +result: u32
        +overflow: bool
        +zero: bool
        +negative: bool
    }
}
    copper-codegen_src_LowerCtx --> copper-core_src_CHIRSubmoduleInst
    copper-codegen_src_IRBuilder ..> copper-core_src_ModuleIR
    copper-codegen_src_IRBuilder ..> copper-codegen_src_LowerError
    copper-core_src_ModuleIR --> copper-core_src_Port
    copper-core_src_ModuleIR --> copper-core_src_ModuleIR
    copper-core_src_Assignment --> copper-core_src_Signal
    copper-core_src_FrontendModuleIR --> copper-core_src_FrontendSignature
    copper-core_src_FrontendModuleIR --> copper-core_src_ClockParamMeta
    copper-core_src_FrontendModuleIR --> copper-core_src_RawStmt
    copper-core_src_FrontendModuleIR --> copper-core_src_SourceSpan
    copper-core_src_FrontendSignature --> copper-core_src_RawParam
    copper-core_src_FrontendSignature --> copper-core_src_RawTypeRef
    copper-core_src_RawParam --> copper-core_src_RawTypeRef
    copper-core_src_RawParam --> copper-core_src_SourceSpan
    copper-core_src_RawTypeRef --> copper-core_src_SourceSpan
    copper-core_src_ClockParamMeta --> copper-core_src_SourceSpan
    copper-core_src_LocalStmt --> copper-core_src_RawTypeRef
    copper-core_src_LocalStmt --> copper-core_src_SourceSpan
    copper-core_src_ItemConst --> copper-core_src_RawTypeRef
    copper-core_src_ItemConst --> copper-core_src_SourceSpan
    copper-core_src_ItemEnum --> copper-core_src_EnumVariant
    copper-core_src_ItemEnum --> copper-core_src_SourceSpan
    copper-core_src_EnumVariant --> copper-core_src_SourceSpan
    copper-core_src_ItemStruct --> copper-core_src_StructField
    copper-core_src_ItemStruct --> copper-core_src_SourceSpan
    copper-core_src_StructField --> copper-core_src_RawTypeRef
    copper-core_src_StructField --> copper-core_src_SourceSpan
    copper-core_src_ItemType --> copper-core_src_RawTypeRef
    copper-core_src_ItemType --> copper-core_src_SourceSpan
    copper-core_src_ItemMacro --> copper-core_src_SourceSpan
    copper-core_src_ItemOther --> copper-core_src_SourceSpan
    copper-core_src_ExprStmt --> copper-core_src_SourceSpan
    copper-core_src_ExprArray --> copper-core_src_SourceSpan
    copper-core_src_ExprAssign --> copper-core_src_SourceSpan
    copper-core_src_ExprAsync --> copper-core_src_RawStmt
    copper-core_src_ExprAsync --> copper-core_src_SourceSpan
    copper-core_src_ExprAwait --> copper-core_src_SourceSpan
    copper-core_src_ExprBinary --> copper-core_src_SourceSpan
    copper-core_src_ExprCall --> copper-core_src_SourceSpan
    copper-core_src_ExprCast --> copper-core_src_RawTypeRef
    copper-core_src_ExprCast --> copper-core_src_SourceSpan
    copper-core_src_ExprField --> copper-core_src_SourceSpan
    copper-core_src_ExprIf --> copper-core_src_RawStmt
    copper-core_src_ExprIf --> copper-core_src_SourceSpan
    copper-core_src_ExprLet --> copper-core_src_SourceSpan
    copper-core_src_ExprLit --> copper-core_src_SourceSpan
    copper-core_src_ExprLoop --> copper-core_src_RawStmt
    copper-core_src_ExprLoop --> copper-core_src_SourceSpan
    copper-core_src_ExprMatchArm --> copper-core_src_SourceSpan
    copper-core_src_ExprMatch --> copper-core_src_ExprMatchArm
    copper-core_src_ExprMatch --> copper-core_src_SourceSpan
    copper-core_src_ExprMethodCall --> copper-core_src_SourceSpan
    copper-core_src_ExprRange --> copper-core_src_SourceSpan
    copper-core_src_ExprReference --> copper-core_src_SourceSpan
    copper-core_src_ExprRepeat --> copper-core_src_SourceSpan
    copper-core_src_ExprReturn --> copper-core_src_SourceSpan
    copper-core_src_ExprUnary --> copper-core_src_SourceSpan
    copper-core_src_ExprWhile --> copper-core_src_RawStmt
    copper-core_src_ExprWhile --> copper-core_src_SourceSpan
    copper-core_src_ExprYield --> copper-core_src_SourceSpan
    copper-core_src_RawStmt --> copper-core_src_SourceSpan
    copper-core_src_Memory --> copper-core_src_Clock
    copper-core_src_SHIRModule --> copper-core_src_SHIRPort
    copper-core_src_SHIRModule --> copper-core_src_SourceSpan
    copper-core_src_SHIRPort --> copper-core_src_SourceSpan
    copper-core_src_SHIRCombBody --> copper-core_src_SHIRSubmoduleInst
    copper-core_src_SHIRCombBody --> copper-core_src_SHIRWire
    copper-core_src_SHIRSeqBody --> copper-core_src_SHIRReg
    copper-core_src_SHIRSeqBody --> copper-core_src_SHIRSubmoduleInst
    copper-core_src_SHIRSeqBody --> copper-core_src_SHIRPhase
    copper-core_src_SHIRReg --> copper-core_src_SHIRLit
    copper-core_src_SHIRPhase --> copper-core_src_SHIRRegUpdate
    copper-core_src_CHIRModule --> copper-core_src_CHIRPort
    copper-core_src_CHIRModule --> copper-core_src_SourceSpan
    copper-core_src_CHIRPort --> copper-core_src_SourceSpan
    copper-core_src_CHIRCombBody --> copper-core_src_CHIRSubmoduleInst
    copper-core_src_CHIRCombBody --> copper-core_src_CHIRWireDecl
    copper-core_src_CHIRWireDecl --> copper-core_src_SourceSpan
    copper-core_src_CHIRSeqBody --> copper-core_src_CHIRRegDecl
    copper-core_src_CHIRSeqBody --> copper-core_src_CHIRSubmoduleInst
    copper-core_src_CHIRRegDecl --> copper-core_src_CHIRLit
    copper-core_src_CHIRRegDecl --> copper-core_src_SourceSpan
    copper-core_src_CHIRSubmoduleInst --> copper-core_src_SourceSpan
    copper-core_src_CHIRMatchArm --> copper-core_src_SourceSpan
    tests_MemoryBackedFifo --> copper-core_src_Clock
    tests_MemoryBackedFifo --> tests_FifoClk
    tests_MemoryBackedFifo --> copper-core_src_Memory
    tests_MemoryBackedFifo --> tests_FifoClk
    tests_MemoryBackedFifo ..> tests_FifoOutputs
    examples_cpu_RType --> examples_cpu_Opcode
    examples_cpu_IType --> examples_cpu_Opcode
    examples_cpu_SType --> examples_cpu_Opcode
    examples_cpu_BType --> examples_cpu_Opcode
    examples_cpu_UType --> examples_cpu_Opcode
    examples_cpu_JType --> examples_cpu_Opcode
    examples_cpu_InstrDecoded --> examples_cpu_Opcode
    examples_cpu_InstrDecoded --> examples_cpu_Opcode
    examples_helpers_BinaryTestRunner --> examples_helpers_RV32IProgram
    copper-sim_src_SimulationTrace --> copper-sim_src_CycleData
    copper-sim_src_HardwareExecutor --> copper-sim_src_TaskEntry
    copper-sim_src_HardwareExecutor --> copper-sim_src_ModuleInfo
    copper-sim_src_HardwareExecutor ..> copper-sim_src_ModuleInfo
    copper-sim_src_HardwareTest --> copper-sim_src_SimulationTrace
    copper-sim_src_HardwareTest --> copper-sim_src_PhasedCycleData
    copper-sim_src_HardwareTest ..> copper-sim_src_TestResult
    copper-sim_src_HardwareTest ..> copper-sim_src_TestResult
    copper-sim_src_HardwareTest ..> copper-sim_src_TestResult
    rv32i_types_examples_cpu_InstrDecoded --> rv32i_types_examples_cpu_Opcode

```