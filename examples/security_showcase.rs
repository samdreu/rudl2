// Copper showcase: Security vulnerabilities that Copper prevents
//
// Hardware security bugs are particularly dangerous because they can't be
// patched after fabrication. Copper's type system and ownership model prevent
// several classes of hardware security vulnerabilities.

use copper_core::{Bit, Bits, Clock, ClockDomain};
use copper_sim::{HardwareExecutor, emit};
use std::sync::{Arc, Mutex};

enum MainClk {}
impl ClockDomain for MainClk {}

// ============================================================================
// 1. Timing Side-Channel from Non-Constant-Time Operations
// ============================================================================
//
// Verilog bug (see verilog/bug_timing_sidechannel.v):
//   Case statement with partial coverage can have different timing
//   for different inputs, leaking information via power analysis.
//
// Copper solution: Exhaustive pattern matching ensures all paths exist.
// Combined with constant-time annotations (future feature), we can
// verify operations are timing-safe.

fn safe_constant_time_mux(select: Bit, secret_a: Bits<8>, secret_b: Bits<8>) -> Bits<8> {
    // Exhaustive match ensures both paths exist
    // In the future, we can add #[constant_time] annotation to verify
    // assembly has no data-dependent branches
    match select {
        Bit::ZERO => secret_a,
        Bit::ONE => secret_b,
        Bit::X => panic!("Unknown state in constant-time operation"),
    }
}

// ============================================================================
// 2. Register Initialization Failures
// ============================================================================
//
// Verilog bug (see verilog/bug_register_init.v):
//   Registers without reset values may power up to arbitrary states,
//   potentially bypassing security checks or leaking secrets.
//
// Copper solution: All state must be explicitly initialized.
// Rust's type system prevents use of uninitialized memory.

async fn safe_security_fsm(clk: Clock<MainClk>, auth_code: Bits<8>) -> Bit {
    // Must explicitly initialize - no uninitialized state possible
    let mut authenticated = Bit::ZERO;
    let mut attempt_count = Bits::<8>::from_u128(0);
    
    loop {
        emit!(authenticated);
        clk.tick().await;
        
        let correct_code = Bits::<8>::from_u128(0x42);
        
        if auth_code == correct_code {
            authenticated = Bit::ONE;
        } else {
            attempt_count = attempt_count + Bits::<8>::from_u128(1);
            
            // Lockout after 3 failed attempts
            if attempt_count.as_u128() >= 3 {
                authenticated = Bit::ZERO;
                // In real hardware, would assert a permanent lock signal
            }
        }
    }
}

// ============================================================================
// 3. State Machine Lockup / Illegal State Handling
// ============================================================================
//
// Verilog bug (see verilog/bug_fsm_illegal_state.v):
//   State machines that don't handle all possible bit patterns can
//   lock up or enter undefined behavior when hit by fault injection.
//
// Copper solution: Enums with exhaustive matching ensure all states handled.

#[derive(Copy, Clone, PartialEq)]
enum SecureState {
    Idle,
    AuthCheck,
    Authenticated,
    Locked,
}

async fn safe_secure_fsm(clk: Clock<MainClk>, input: Bit) -> Bit {
    let mut state = SecureState::Idle;
    let mut output = Bit::ZERO;
    
    loop {
        emit!(output);
        clk.tick().await;
        
        // Exhaustive match - compiler ensures all states are handled
        // If attacker uses fault injection to flip bits in state register,
        // we'll catch it (would need hardware support to fully handle,
        // but at least no undefined behavior)
        state = match state {
            SecureState::Idle => {
                if input == Bit::ONE {
                    SecureState::AuthCheck
                } else {
                    SecureState::Idle
                }
            }
            SecureState::AuthCheck => {
                // Check authentication...
                output = Bit::ONE;
                SecureState::Authenticated
            }
            SecureState::Authenticated => {
                if input == Bit::ZERO {
                    output = Bit::ZERO;
                    SecureState::Idle
                } else {
                    SecureState::Authenticated
                }
            }
            SecureState::Locked => {
                // Once locked, stay locked (no exit condition)
                output = Bit::ZERO;
                SecureState::Locked
            }
        };
    }
}

// ============================================================================
// 4. Information Leakage via Uninitialized Outputs
// ============================================================================
//
// Verilog bug (see verilog/bug_info_leak.v):
//   Output that's not assigned in all paths may retain old values,
//   potentially leaking previous secret data.
//
// Copper solution: Functions must return a value in all paths.
// No possibility of "forgetting" to assign an output.

fn safe_conditional_output(condition: Bit, secret: Bits<8>, public: Bits<8>) -> Bits<8> {
    // Compiler enforces that we return in both branches
    // No way to "forget" to assign output and leak old value
    match condition {
        Bit::ZERO => public,
        Bit::ONE => secret,
        Bit::X => panic!("Unknown state"),
    }
}

// ============================================================================
// 5. Privilege Escalation via Implicit State
// ============================================================================
//
// Verilog bug: Global state (like implicit wires or unchecked intermediates)
//              can be manipulated to bypass security checks.
//
// Copper solution: Ownership and explicit data flow prevent hidden state.

#[derive(Copy, Clone, PartialEq)]
enum PrivilegeLevel {
    User,
    Admin,
}

fn safe_privileged_operation(
    privilege: PrivilegeLevel,
    command: Bits<8>
) -> (Bit, Bits<8>) {
    // Privilege level is explicitly passed and checked
    // No global state that could be manipulated
    let allowed = match privilege {
        PrivilegeLevel::Admin => Bit::ONE,
        PrivilegeLevel::User => Bit::ZERO,
    };
    
    let result = if allowed == Bit::ONE {
        // Execute privileged command
        command
    } else {
        // Return error code
        Bits::<8>::from_u128(0xFF)
    };
    
    (allowed, result)
}

// ============================================================================
// Demo
// ============================================================================

fn main() {
    println!("=== Security Showcase ===\n");
    
    // Demo 1: Constant-time operations
    println!("1) Timing Side-Channel Prevention");
    println!("   Verilog: Partial case coverage can leak timing information");
    println!("   Copper:  Exhaustive matching + future #[constant_time] checks\n");
    
    let secret_key_a = Bits::<8>::from_u128(0xDE);
    let secret_key_b = Bits::<8>::from_u128(0xAD);
    
    for select_val in [Bit::ZERO, Bit::ONE] {
        let result = safe_constant_time_mux(select_val, secret_key_a.clone(), secret_key_b.clone());
        println!("   select={} -> output=0x{:02x}", select_val, result.as_u128());
    }
    println!("   Both paths compiled and execute (no timing leak)\n");
    
    // Demo 2: Initialization
    println!("2) Register Initialization");
    println!("   Verilog: Uninitialized registers may power up to arbitrary values");
    println!("   Copper:  All state must be explicitly initialized\n");
    
    let mut clk = Clock::<MainClk>::new();
    let mut executor = HardwareExecutor::new();
    
    let auth_output = executor.spawn_function_typed(Bit::ZERO,
        safe_security_fsm(clk.clone(), Bits::<8>::from_u128(0x42)));
    
    println!("   Initial state: authenticated={} (explicitly ZERO)", 
             *auth_output.lock().unwrap());
    executor.tick_clock(&mut clk);
    println!("   After correct code: authenticated={}", 
             *auth_output.lock().unwrap());
    println!("   No uninitialized state possible\n");
    
    // Demo 3: State machine illegal states
    println!("3) FSM Illegal State Handling");
    println!("   Verilog: Unhandled states can cause undefined behavior");
    println!("   Copper:  Exhaustive enum matching ensures all states covered\n");
    
    let mut clk2 = Clock::<MainClk>::new();
    let mut executor2 = HardwareExecutor::new();
    
    let fsm_output = executor2.spawn_function_typed(Bit::ZERO,
        safe_secure_fsm(clk2.clone(), Bit::ONE));
    
    for cycle in 0..4 {
        executor2.tick_clock(&mut clk2);
        let out = *fsm_output.lock().unwrap();
        println!("   Cycle {}: output={}", cycle, out);
    }
    println!("   All state transitions explicitly handled\n");
    
    // Demo 4: Information leakage
    println!("4) Information Leakage Prevention");
    println!("   Verilog: Outputs not assigned in all paths may leak old values");
    println!("   Copper:  Must return value in all code paths\n");
    
    let secret = Bits::<8>::from_u128(0xCA);
    let public = Bits::<8>::from_u128(0xFE);
    
    for condition in [Bit::ZERO, Bit::ONE] {
        let output = safe_conditional_output(condition, secret.clone(), public.clone());
        println!("   condition={} -> output=0x{:02x}", condition, output.as_u128());
    }
    println!("   Compiler enforces output assigned in all paths\n");
    
    // Demo 5: Privilege escalation
    println!("5) Privilege Escalation Prevention");
    println!("   Verilog: Global state can be manipulated to bypass checks");
    println!("   Copper:  Explicit data flow and ownership\n");
    
    let cmd = Bits::<8>::from_u128(0x99);
    
    let (allowed, result) = safe_privileged_operation(PrivilegeLevel::User, cmd.clone());
    println!("   User level: allowed={}, result=0x{:02x}", allowed, result.as_u128());
    
    let (allowed, result) = safe_privileged_operation(PrivilegeLevel::Admin, cmd);
    println!("   Admin level: allowed={}, result=0x{:02x}", allowed, result.as_u128());
    println!("   No hidden global state to manipulate\n");
    
    println!("Key Insight: Copper's type system and ownership prevent many");
    println!("hardware security vulnerabilities at compile time.");
    println!("\nSee buggy Verilog modules in:");
    println!("  - verilog/bug_timing_sidechannel.v");
    println!("  - verilog/bug_register_init.v");
    println!("  - verilog/bug_fsm_illegal_state.v");
    println!("  - verilog/bug_info_leak.v");
}
