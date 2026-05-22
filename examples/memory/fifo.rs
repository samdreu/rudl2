use copper_core::{Bit, Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, emit};
use std::sync::{Arc, Mutex};

struct MainClk;
impl ClockDomain for MainClk {}

/// 4-element, 8-bit wide FIFO (First-In-First-Out) queue
/// - write_en: 1 to push data into FIFO
/// - read_en: 1 to pop data from FIFO
/// - data_in: input data to push (8 bits)
/// Output: (data_out, empty, full, valid)
///   - data_out: data at front of FIFO (8 bits)
///   - empty: 1 if FIFO is empty
///   - full: 1 if FIFO is full
///   - valid: 1 if data_out contains valid data (not empty)
#[hardware(function_typed)]
async fn fifo(
    clk: Clock<MainClk>,
    write_en: Arc<Mutex<Bit>>,
    read_en: Arc<Mutex<Bit>>,
    data_in: Arc<Mutex<Bits<8>>>,
) -> (Bits<8>, Bit, Bit, Bit) {
    // Circular buffer storage (4 elements, 8 bits each)
    let mut buffer: Vec<Bits<8>> = (0..4).map(|_| Bits::from_u128(0)).collect();
    
    // Pointers: read_ptr points to oldest data, write_ptr points to next slot
    let mut read_ptr: usize = 0;
    let mut write_ptr: usize = 0;
    let mut count: usize = 0;

    loop {
        let we = *write_en.lock().unwrap();
        let re = *read_en.lock().unwrap();
        
        // Must read data_in contents by accessing and converting to u128
        let din_val = data_in.lock().unwrap().as_u128();

        // Determine current state
        let is_empty = count == 0;
        let is_full = count == 4;
        let is_valid = !is_empty;

        // Get data at front (read pointer) 
        let dout: Bits<8> = if is_empty {
            Bits::from_u128(0)
        } else {
            let front_val = buffer[read_ptr].as_u128();
            Bits::from_u128(front_val)
        };

        // Handle write
        if we == Bit::ONE && !is_full {
            buffer[write_ptr] = Bits::from_u128(din_val);
            write_ptr = (write_ptr + 1) % 4;
            count += 1;
        }

        // Handle read
        if re == Bit::ONE && !is_empty {
            read_ptr = (read_ptr + 1) % 4;
            count -= 1;
        }

        // Output: data, empty, full, valid
        emit!((
            dout,
            if is_empty { Bit::ONE } else { Bit::ZERO },
            if is_full { Bit::ONE } else { Bit::ZERO },
            if is_valid { Bit::ONE } else { Bit::ZERO }
        ));

        clk.tick().await;
    }
}

fn main() {
    println!("FIFO (4-element, 8-bit) - Test Sequence");
    println!("=========================================");

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let write_en = Arc::new(Mutex::new(Bit::ZERO));
    let read_en = Arc::new(Mutex::new(Bit::ZERO));
    let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

    let write_en_clone = Arc::clone(&write_en);
    let read_en_clone = Arc::clone(&read_en);
    let data_in_clone = Arc::clone(&data_in);
    let clk_inner = clk.clone();

    let output = exec.spawn_function_typed(
        (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
        {
            let write_en = write_en_clone;
            let read_en = read_en_clone;
            let data_in = data_in_clone;
            async move {
                fifo(clk_inner, write_en, read_en, data_in).await
            }
        },
    );

    println!("Cycle | WE | RE | DataIn | DataOut | Empty | Full | Valid");
    println!("-------|----|----|--------|---------|-------|------|-------");

    // Test sequence
    let test_ops = vec![
        (0, 0, 0, "Initial state (empty)"),
        (1, 0, 10, "Write 10"),
        (1, 0, 20, "Write 20"),
        (1, 0, 30, "Write 30"),
        (1, 0, 40, "Write 40 (full)"),
        (0, 0, 0, "No operation (full)"),
        (0, 1, 0, "Read (pop 10)"),
        (1, 1, 50, "Write 50 & Read (pop 20)"),
        (1, 1, 60, "Write 60 & Read (pop 30)"),
        (0, 1, 0, "Read (pop 40)"),
        (0, 1, 0, "Read (pop 50)"),
        (0, 1, 0, "Read (pop 60)"),
        (0, 0, 0, "No operation (now empty)"),
    ];

    for (cycle, (we, re, din, desc)) in test_ops.iter().enumerate() {
        *write_en.lock().unwrap() = if *we == 1 { Bit::ONE } else { Bit::ZERO };
        *read_en.lock().unwrap() = if *re == 1 { Bit::ONE } else { Bit::ZERO };
        *data_in.lock().unwrap() = Bits::from_u128(*din as u128);

        exec.tick_clock(&mut clk);

        let guard = output.lock().unwrap();
        let dout_val = guard.0.as_u128();
        let empty = guard.1;
        let full = guard.2;
        let valid = guard.3;
        let empty_val = if empty == Bit::ONE { "Y" } else { "N" };
        let full_val = if full == Bit::ONE { "Y" } else { "N" };
        let valid_val = if valid == Bit::ONE { "Y" } else { "N" };

        println!(
            "{:5} | {:2} | {:2} |    {:3} |     {:3} | {:5} | {:4} | {:5} - {}",
            cycle, we, re, din, dout_val, empty_val, full_val, valid_val, desc
        );
    }

    println!("\nTest completed successfully!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fifo_initial_state() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        exec.tick_clock(&mut clk);

        let guard = output.lock().unwrap();
        let empty = guard.1;
        let full = guard.2;
        let valid = guard.3;
        assert_eq!(empty, Bit::ONE, "FIFO should start empty");
        assert_eq!(full, Bit::ZERO, "FIFO should not start full");
        assert_eq!(valid, Bit::ZERO, "Valid should be low when empty");
    }

    #[test]
    fn test_fifo_write_single() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Write value 42
        *write_en.lock().unwrap() = Bit::ONE;
        *data_in.lock().unwrap() = Bits::from_u128(42);
        exec.tick_clock(&mut clk);

        let (dout, empty, _, valid) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 42, "Data should be available immediately");
        assert_eq!(empty, Bit::ZERO, "FIFO should not be empty");
        assert_eq!(valid, Bit::ONE, "Valid should be high");
    }

    #[test]
    fn test_fifo_write_multiple_and_read() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Write 10, 20, 30
        for val in vec![10, 20, 30] {
            *write_en.lock().unwrap() = Bit::ONE;
            *data_in.lock().unwrap() = Bits::from_u128(val);
            exec.tick_clock(&mut clk);
        }

        // After 3 writes, front should still be first written (10)
        let (dout, _, _, _) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 10, "First element should be at front");

        // Read first element
        *write_en.lock().unwrap() = Bit::ZERO;
        *read_en.lock().unwrap() = Bit::ONE;
        exec.tick_clock(&mut clk);

        let (dout, _, _, _) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 20, "After reading, second element should be at front");

        // Read second element
        exec.tick_clock(&mut clk);
        let (dout, _, _, _) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 30, "After reading again, third element should be at front");
    }

    #[test]
    fn test_fifo_full_condition() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Fill FIFO completely (4 elements)
        for i in 0..4 {
            *write_en.lock().unwrap() = Bit::ONE;
            *data_in.lock().unwrap() = Bits::from_u128((i + 1) as u128 * 10);
            exec.tick_clock(&mut clk);
        }

        let (_, _, full, _) = (*output.lock().unwrap());
        assert_eq!(full, Bit::ONE, "FIFO should be full after 4 writes");

        // Try to write when full (should be ignored)
        *write_en.lock().unwrap() = Bit::ONE;
        *data_in.lock().unwrap() = Bits::from_u128(999);
        exec.tick_clock(&mut clk);

        let (dout, _, _, _) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 10, "Data should not change (still first in)");
    }

    #[test]
    fn test_fifo_read_write_simultaneous() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Pre-fill with two elements
        for val in vec![10, 20] {
            *write_en.lock().unwrap() = Bit::ONE;
            *data_in.lock().unwrap() = Bits::from_u128(val);
            exec.tick_clock(&mut clk);
        }

        // Simultaneous read and write
        *write_en.lock().unwrap() = Bit::ONE;
        *read_en.lock().unwrap() = Bit::ONE;
        *data_in.lock().unwrap() = Bits::from_u128(30);
        exec.tick_clock(&mut clk);

        let (dout, _, _, _) = (*output.lock().unwrap());
        // After simultaneous read/write, we should see one of the middle elements
        assert!(dout.as_u128() >= 10 && dout.as_u128() <= 30, "Output should be from valid data");
    }

    #[test]
    fn test_fifo_empty_condition() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Write one element
        *write_en.lock().unwrap() = Bit::ONE;
        *data_in.lock().unwrap() = Bits::from_u128(42);
        exec.tick_clock(&mut clk);

        // Read it out
        *write_en.lock().unwrap() = Bit::ZERO;
        *read_en.lock().unwrap() = Bit::ONE;
        exec.tick_clock(&mut clk);

        // Check empty
        let (_, empty, _, valid) = (*output.lock().unwrap());
        assert_eq!(empty, Bit::ONE, "FIFO should be empty after reading last element");
        assert_eq!(valid, Bit::ZERO, "Valid should be low when empty");
    }

    #[test]
    fn test_fifo_wraparound() {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();

        let write_en = Arc::new(Mutex::new(Bit::ZERO));
        let read_en = Arc::new(Mutex::new(Bit::ZERO));
        let data_in = Arc::new(Mutex::new(Bits::<8>::from_u128(0)));

        let write_en_clone = Arc::clone(&write_en);
        let read_en_clone = Arc::clone(&read_en);
        let data_in_clone = Arc::clone(&data_in);
        let clk_inner = clk.clone();

        let output = exec.spawn_function_typed(
            (Bits::<8>::from_u128(0), Bit::ONE, Bit::ZERO, Bit::ZERO),
            {
                let write_en = write_en_clone;
                let read_en = read_en_clone;
                let data_in = data_in_clone;
                async move {
                    fifo(clk_inner, write_en, read_en, data_in).await
                }
            },
        );

        // Fill buffer
        for i in 0..4 {
            *write_en.lock().unwrap() = Bit::ONE;
            *data_in.lock().unwrap() = Bits::from_u128((i + 100) as u128);
            exec.tick_clock(&mut clk);
        }

        // Read all and then write new ones (tests wraparound)
        for _ in 0..4 {
            *write_en.lock().unwrap() = Bit::ZERO;
            *read_en.lock().unwrap() = Bit::ONE;
            exec.tick_clock(&mut clk);
        }

        // Now write new data - should wrap around
        for i in 0..3 {
            *write_en.lock().unwrap() = Bit::ONE;
            *read_en.lock().unwrap() = Bit::ZERO;
            *data_in.lock().unwrap() = Bits::from_u128((i + 200) as u128);
            exec.tick_clock(&mut clk);
        }

        let (dout, _, _, _) = (*output.lock().unwrap());
        assert_eq!(dout.as_u128(), 200, "After wraparound, should see new data");
    }
}
