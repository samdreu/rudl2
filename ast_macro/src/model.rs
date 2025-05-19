pub struct FieldAction {
    pub field_name: String,
    pub field_type: String,
    pub actions: Vec<MethodAction>,
}

pub struct MethodAction {
    pub method_name: String,
    pub arguments: Vec<String>,
}
