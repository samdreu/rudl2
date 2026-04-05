use copper_core::ir::{Direction, PortType};
use syn::ItemFn;

use super::IRBuilder;

fn parse_fn(code: &str) -> ItemFn {
    syn::parse_str::<ItemFn>(code).expect("failed to parse function")
}

#[test]
fn extract_ports_sync_single_output_defaults_to_out() {
    let func = parse_fn(
        r#"
        fn inverter(bit: Bit) -> Bit {
            !bit
        }
        "#,
    );

    let ports = IRBuilder::extract_ports(&func, false).expect("extract_ports failed");

    assert_eq!(ports.len(), 2);

    assert_eq!(ports[0].name, "bit_sig");
    assert_eq!(ports[0].direction, Direction::Input);
    assert_eq!(ports[0].width, 1);
    assert_eq!(ports[0].ty, PortType::Wire);

    assert_eq!(ports[1].name, "out");
    assert_eq!(ports[1].direction, Direction::Output);
    assert_eq!(ports[1].width, 1);
    assert_eq!(ports[1].ty, PortType::Wire);
}

#[test]
fn extract_ports_sync_tuple_output_dedupes_names() {
    let func = parse_fn(
        r#"
        fn split(a: Bit, b: Bits<4>) -> (Bit, Bits<4>) {
            (a, b)
        }
        "#,
    );

    let ports = IRBuilder::extract_ports(&func, false).expect("extract_ports failed");

    assert_eq!(ports.len(), 4);

    assert_eq!(ports[0].name, "a");
    assert_eq!(ports[0].direction, Direction::Input);
    assert_eq!(ports[0].width, 1);

    assert_eq!(ports[1].name, "b");
    assert_eq!(ports[1].direction, Direction::Input);
    assert_eq!(ports[1].width, 4);

    assert_eq!(ports[2].name, "a_out");
    assert_eq!(ports[2].direction, Direction::Output);
    assert_eq!(ports[2].width, 1);
    assert_eq!(ports[2].ty, PortType::Wire);

    assert_eq!(ports[3].name, "b_out");
    assert_eq!(ports[3].direction, Direction::Output);
    assert_eq!(ports[3].width, 4);
    assert_eq!(ports[3].ty, PortType::Wire);
}

#[test]
fn extract_ports_sync_skips_clock_param() {
    let func = parse_fn(
        r#"
        fn pass_through(clk: Clock<MainClk>, data: Bit) -> Bit {
            !data
        }
        "#,
    );

    let ports = IRBuilder::extract_ports(&func, false).expect("extract_ports failed");

    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0].name, "data");
    assert_eq!(ports[0].direction, Direction::Input);
    assert_eq!(ports[0].width, 1);

    assert_eq!(ports[1].name, "out");
    assert_eq!(ports[1].direction, Direction::Output);
    assert_eq!(ports[1].width, 1);
}

#[test]
fn extract_ports_async_collects_emit_outputs_and_skips_output_handle() {
    let func = parse_fn(
        r#"
        async fn registered(
            clk: Clock<MainClk>,
            input: Arc<Mutex<Bit>>,
            output: Arc<Mutex<Bit>>,
        ) {
            loop {
                emit!(out, input);
                clk.tick().await;
            }
        }
        "#,
    );

    let ports = IRBuilder::extract_ports(&func, true).expect("extract_ports failed");

    assert_eq!(ports.len(), 3);

    assert_eq!(ports[0].name, "clk");
    assert_eq!(ports[0].direction, Direction::Input);
    assert_eq!(ports[0].width, 1);
    assert_eq!(ports[0].ty, PortType::Wire);

    assert_eq!(ports[1].name, "input_sig");
    assert_eq!(ports[1].direction, Direction::Input);
    assert_eq!(ports[1].width, 1);
    assert_eq!(ports[1].ty, PortType::Wire);

    assert_eq!(ports[2].name, "out");
    assert_eq!(ports[2].direction, Direction::Output);
    assert_eq!(ports[2].width, 1);
    assert_eq!(ports[2].ty, PortType::Reg);
}
