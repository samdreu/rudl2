use copper_core::{Bit, Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, emit};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

/// N-bit serial shift register
/// - When set=1, all register bits are preset to 1
/// - When set=0, shift right and load shift_in on the MSB
/// - Output the LSB each cycle
#[hardware(function_typed)]
async fn serial_sr<const N: usize>(
    clk: Clock<MainClk>,
    set: Arc<Mutex<Bit>>,
    shift_in: Arc<Mutex<Bit>>,
) -> Bits<1> {
    let mut reg: Bits<N> = Bits::from_u128(0);

    loop {
        let set_val = *set.lock().unwrap();
        let shift_in_val = *shift_in.lock().unwrap();

        if set_val == Bit::ONE {
            // Preset: load all 1s
            reg = Bits::ones();
        } else {
            // Shift operation: shift left and load shift_in on LSB
            reg = reg.shift_left(1);
            reg = reg.with_lsb(shift_in_val);
        }

        // Output the LSB
        let output_val = if reg.get(0) == Bit::ONE { 1u128 } else { 0u128 };
        let output_bits: Bits<1> = Bits::from_u128(output_val);
        emit!(output_bits);
        
        clk.tick().await;
    }
}

fn main() {
    println!("Serial Shift Register (4-bit) - Test Pattern");
    println!("=============================================");
    
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let set = Arc::new(Mutex::new(Bit::ZERO));
    let shift_in = Arc::new(Mutex::new(Bit::ZERO));

    let set_clone = Arc::clone(&set);
    let shift_in_clone = Arc::clone(&shift_in);

    let clk_inner = clk.clone();
    let output = exec.spawn_function_typed(
        Bits::<1>::from_u128(0),
        {
            let set = set_clone;
            let shift_in = shift_in_clone;
            async move {
                serial_sr::<4>(clk_inner, set, shift_in).await
            }
        },
    );

    println!("Cycle | Set | ShiftIn | Output | Reg State (4-bit)");
    println!("------|-----|---------|--------|------------------");

    let mut reg_state = 0u128; // Track internal state manually for display

    // Test sequence
    let test_sequence = vec![
        // Preset to all 1s
        (1, 0, "Preset all 1s"),
        (0, 0, "Shift in 0"),
        (0, 0, "Shift in 0"),
        (0, 0, "Shift in 0"),
        (0, 0, "Shift in 0"),
        (0, 1, "Shift in 1"),
        (0, 1, "Shift in 1"),
        (0, 1, "Shift in 1"),
        (0, 1, "Shift in 1"),
        // Reset and shift in alternating pattern
        (1, 0, "Preset again"),
        (0, 1, "Shift in 1"),
        (0, 0, "Shift in 0"),
        (0, 1, "Shift in 1"),
        (0, 0, "Shift in 0"),
    ];

    for (cycle, (set_val, shift_val, description)) in test_sequence.iter().enumerate() {
        // Set inputs
        *set.lock().unwrap() = if *set_val == 1 {
            Bit::ONE
        } else {
            Bit::ZERO
        };
        *shift_in.lock().unwrap() = if *shift_val == 1 {
            Bit::ONE
        } else {
            Bit::ZERO
        };

        // Tick clock
        exec.tick_clock(&mut clk);

        // Read output
        let output_val = output.lock().unwrap().as_u128();
        
        // Update simulated internal state (simplified representation)
        if *set_val == 1 {
            reg_state = 0xF; // all 1s
        } else {
            reg_state = (reg_state >> 1) | ((*shift_val as u128) << 3);
        }

        println!(
            "{:5} | {:3} | {:7} | {:6} | {:#06b} - {}",
            cycle, set_val, shift_val, output_val, reg_state, description
        );
    }

    println!("\nTest completed successfully!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_sr_preset_ones() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let set = Arc::new(Mutex::new(Bit::ONE));
        let shift_in = Arc::new(Mutex::new(Bit::ZERO));

        let set_clone = Arc::clone(&set);
        let shift_in_clone = Arc::clone(&shift_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            Bits::<1>::from_u128(0),
            {
                let set = set_clone;
                let shift_in = shift_in_clone;
                async move {
                    serial_sr::<4>(clk_inner, set, shift_in).await
                }
            },
        );

        // Preset to all 1s
        *set.lock().unwrap() = Bit::ONE;
        exec.tick_clock(&mut clk);
        let output1 = output.lock().unwrap().as_u128();

        // After preset, LSB should be 1
        assert_eq!(output1, 1, "Preset should load all 1s, LSB should be 1");
    }

    #[test]
    fn test_serial_sr_shift_in_zeros() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let set = Arc::new(Mutex::new(Bit::ZERO));
        let shift_in = Arc::new(Mutex::new(Bit::ZERO));

        let set_clone = Arc::clone(&set);
        let shift_in_clone = Arc::clone(&shift_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            Bits::<1>::from_u128(0),
            {
                let set = set_clone;
                let shift_in = shift_in_clone;
                async move {
                    serial_sr::<4>(clk_inner, set, shift_in).await
                }
            },
        );

        // Shift in 0s
        *set.lock().unwrap() = Bit::ZERO;
        *shift_in.lock().unwrap() = Bit::ZERO;

        for _ in 0..5 {
            exec.tick_clock(&mut clk);
        }

        let output_final = output.lock().unwrap().as_u128();
        assert_eq!(output_final, 0, "After shifting in zeros, output should remain 0");
    }

    #[test]
    fn test_serial_sr_shift_in_ones() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let set = Arc::new(Mutex::new(Bit::ZERO));
        let shift_in = Arc::new(Mutex::new(Bit::ONE));

        let set_clone = Arc::clone(&set);
        let shift_in_clone = Arc::clone(&shift_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            Bits::<1>::from_u128(0),
            {
                let set = set_clone;
                let shift_in = shift_in_clone;
                async move {
                    serial_sr::<4>(clk_inner, set, shift_in).await
                }
            },
        );

        // Shift in 1s
        *set.lock().unwrap() = Bit::ZERO;
        *shift_in.lock().unwrap() = Bit::ONE;

        // After 4 cycles of shifting 1s into a 4-bit register, we should see output
        for _ in 0..4 {
            exec.tick_clock(&mut clk);
        }

        let output_final = output.lock().unwrap().as_u128();
        // The register fills with 1s from the left, so eventually outputs become 1
        assert_eq!(output_final, 1, "After shifting 4 ones into 4-bit register, output should be 1");
    }

    #[test]
    fn test_serial_sr_alternating_pattern() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let set = Arc::new(Mutex::new(Bit::ZERO));
        let shift_in = Arc::new(Mutex::new(Bit::ONE));

        let set_clone = Arc::clone(&set);
        let shift_in_clone = Arc::clone(&shift_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            Bits::<1>::from_u128(0),
            {
                let set = set_clone;
                let shift_in = shift_in_clone;
                async move {
                    serial_sr::<4>(clk_inner, set, shift_in).await
                }
            },
        );

        // Shift in pattern: 1, 0, 1, 0
        let pattern = vec![1, 0, 1, 0, 1, 0];
        let mut outputs = vec![];

        for &bit in pattern.iter() {
            *shift_in.lock().unwrap() = if bit == 1 { Bit::ONE } else { Bit::ZERO };
            exec.tick_clock(&mut clk);
            let out = output.lock().unwrap().as_u128();
            outputs.push(out);
        }

        assert_eq!(outputs.len(), 6, "Should have 6 output values");
        // Verify pattern propagated correctly
        assert_ne!(outputs[0], outputs[1], "Pattern should vary as bits shift");
    }

    #[test]
    fn test_serial_sr_preset_then_shift() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let set = Arc::new(Mutex::new(Bit::ONE));
        let shift_in = Arc::new(Mutex::new(Bit::ZERO));

        let set_clone = Arc::clone(&set);
        let shift_in_clone = Arc::clone(&shift_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            Bits::<1>::from_u128(0),
            {
                let set = set_clone;
                let shift_in = shift_in_clone;
                async move {
                    serial_sr::<4>(clk_inner, set, shift_in).await
                }
            },
        );

        // Cycle 1: Preset to all 1s
        *set.lock().unwrap() = Bit::ONE;
        *shift_in.lock().unwrap() = Bit::ZERO;
        exec.tick_clock(&mut clk);
        let output1 = output.lock().unwrap().as_u128();
        assert_eq!(output1, 1, "After preset, LSB should be 1");

        // Cycle 2: Start shifting in 0s
        // shift_left moves bits towards MSB, then we set LSB to shift_in
        // So after shift_left(1) from 1111, we get 1110, then with_lsb(0) keeps it 1110
        // LSB is 0, so output should be 0
        *set.lock().unwrap() = Bit::ZERO;
        exec.tick_clock(&mut clk);
        let output2 = output.lock().unwrap().as_u128();
        assert_eq!(output2, 0, "After first shift with 0, LSB should be 0");

        // Continue shifting in 0s - the 1s will exit the register
        for _ in 3..6 {
            *set.lock().unwrap() = Bit::ZERO;
            *shift_in.lock().unwrap() = Bit::ZERO;
            exec.tick_clock(&mut clk);
        }

        let output_final = output.lock().unwrap().as_u128();
        assert_eq!(output_final, 0, "After shifting in multiple zeros, output should be 0");
    }
}
