use strands::{error::BoxError, tool, ToolError};

fn helper(path: &str) -> Result<String, BoxError> {
    Ok(path.parse::<u32>()?.to_string())
}

/// Read a number.
#[tool]
fn read_number(
    /// The text to parse.
    path: String,
) -> Result<String, ToolError> {
    let number = helper(&path).map_err(ToolError::from_boxed)?;
    Ok(format!("read {number}"))
}

/// Read a number, converting the boxed error by hand.
#[tool]
fn read_boxed(
    /// The text to parse.
    path: String,
) -> Result<String, ToolError> {
    helper(&path).map_err(ToolError::from_boxed)
}

fn main() {
    use strands::Tool;
    assert_eq!(ReadNumber.name(), "read_number");
    assert_eq!(ReadBoxed.name(), "read_boxed");
    let error = ToolError::from_boxed(helper("x").unwrap_err());
    assert!(error.downcast_ref::<std::num::ParseIntError>().is_some());
    assert!(error.interrupt().is_none());
}
