mod register;
mod reg_design;

use reg_design::RegDesign;

fn main() {
    RegDesign::to_verilog();
    RegDesign::to_verilog_assignments();
    let mut design = RegDesign::new();
    design.update();
}
