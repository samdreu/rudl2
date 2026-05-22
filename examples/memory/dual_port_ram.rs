// Single dual-port RAM with single clock


struct MainClk;
impl ClockDomain for MainClk {}

// out is data_out_b
#[hardware(function_typed)]
async fn dual_port_ram(
    clk: Clock<MainClk>,
    enable_a: Arc<Mutex<Bit>>,
    enable_b: Arc<Mutex<Bit>>,
    write_a: Arc<Mutex<Bit>>,
    addr_a: Arc<Mutex<u8>>,
    addr_b: Arc<Mutex<u8>>,
    data_in_a: Arc<Mutex<u16>>,
 ) -> u16 {
    let mem = Memory::<u16, 1, 1, MainClk, 5, 6>::new(clk.clone(), 2^8);
    let mut data_out_b: u16 = 0;
    loop {
        if enable_a == Bit::ONE & write_a == Bit::ONE {
            mem.write_port::<0>().write((*addr_a.lock().unwrap(), &data_in_a.lock().unwrap()));
        }

        if enable_b == Bit::ONE {
            let data_out_b = mem.read_port::<0>().read(*addr_b.lock().unwrap());
            emit!(data_out_b);
        }


    }

}
