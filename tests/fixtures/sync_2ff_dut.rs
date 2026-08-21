// Concrete (transpilable) specialization of the standard-library synchronizer
// `copper::sync_2ff` (`src/sync.rs`). Body kept character-for-character identical
// to the library module — only the generic domain parameters are pinned, because
// `copper-transpile` only handles concrete modules.
//
// `tests/cdc_synchronizer_anchor.rs` asserts this specialization is behaviourally
// identical to the library generic in the simulator, so anchoring this concrete
// module anchors the library primitive.
//
// Domains `SrcClk` / `DstClk` are declared by the including test file.

#[hardware(synchronizer)]
async fn sync_2ff_concrete(clk: Clock<DstClk>, d: In<Logic, SrcClk>, q: Out<Logic, DstClk>) {
    let mut ff1 = Logic::Zero;
    let mut ff2 = Logic::Zero;
    loop {
        q.write(ff2);
        clk.tick().await;
        // ff2 captures the OLD ff1 (ff1 is reassigned after), so the two stages
        // stay distinct — reversing these lines would collapse them into one flop.
        ff2 = ff1;
        ff1 = d.read();
    }
}
