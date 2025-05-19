#[macro_export]
macro_rules! describe_module {
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident {
            $($field:vis $fname:ident : $fty:ty),* $(,)?
        }

        impl $impname:ident {
            $(
                fn $mname:ident $msig:tt $mbody:block
            )*
        }
    ) => {
        $(#[$struct_meta])*
        $vis struct $name {
            $( $field $fname : $fty ),*
        }

        impl $name {
            $(
                fn $mname $msig $mbody
            )*

            pub fn describe_module() {
                println!("Module: {}", stringify!($name));
                $(
                    println!("  Field: {}: {}", stringify!($fname), stringify!($fty));
                )*
            }

            pub fn describe_actions() {
                $(
                    println!("Method: {}", stringify!($mname));
                )*
            }
        }
    }
}
