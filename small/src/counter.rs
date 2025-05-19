pub struct Counter<const N: usize> {
    pub count: Register<N>,
    pub enable: Wire<1>,
}


impl<const N: usize> Counter<N> {
    fn new() -> Self {
        Self {
            count: Register::new(),
            enable: Wire::new(),
        }
    }

    fn design(&mut self) {
        if (self.enable.read() == 1) {
            self.count.write(self.count.read() + 1);
        }
    }
}
