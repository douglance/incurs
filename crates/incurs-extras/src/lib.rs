//! Opt-in output formats that the default incurs CLI surface does not expose.

use serde_json::Value;

/// Output formats excluded from the default CLI surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraFormat {
    /// Aligned ASCII table output.
    Table,
    /// Comma-separated value output.
    Csv,
}

impl ExtraFormat {
    /// Formats a JSON value using this extension format.
    pub fn format(self, value: &Value) -> String {
        match self {
            Self::Table => incurs::formatter::format_table(value),
            Self::Csv => incurs::formatter::format_csv(value),
        }
    }

    /// Returns the corresponding core format for explicitly configured CLI defaults.
    pub fn core(self) -> incurs::output::Format {
        match self {
            Self::Table => incurs::output::Format::Table,
            Self::Csv => incurs::output::Format::Csv,
        }
    }
}

/// Extension methods for explicitly opting a CLI into these formats.
pub trait CliExtras {
    /// Sets a default output format without changing the built-in format flags.
    fn default_extra_format(self, format: ExtraFormat) -> Self;
}

impl CliExtras for incurs::cli::Cli {
    fn default_extra_format(self, format: ExtraFormat) -> Self {
        self.enable_extra_formats([ExtraFormat::Table.core(), ExtraFormat::Csv.core()])
            .format(format.core())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_available_only_through_explicit_extension_api() {
        let value = serde_json::json!([{ "name": "Ada", "active": true }]);
        assert!(ExtraFormat::Table.format(&value).contains("Ada"));
        assert_eq!(ExtraFormat::Csv.format(&value), "name,active\nAda,true");
    }

    #[tokio::test]
    async fn extension_enables_table_and_csv_cli_values() {
        let cli = incurs::cli::Cli::create("demo").default_extra_format(ExtraFormat::Table);
        let mut table_output = Vec::new();
        let table = cli
            .serve_to(
                vec!["--format".into(), "table".into()],
                &mut table_output,
                false,
            )
            .await
            .expect("table invocation");
        let mut csv_output = Vec::new();
        let csv = cli
            .serve_to(
                vec!["--format".into(), "csv".into()],
                &mut csv_output,
                false,
            )
            .await
            .expect("csv invocation");
        assert_ne!(table, Some(1), "{}", String::from_utf8_lossy(&table_output));
        assert_ne!(csv, Some(1), "{}", String::from_utf8_lossy(&csv_output));
    }
}
