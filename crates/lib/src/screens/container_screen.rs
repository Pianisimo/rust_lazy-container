use std::process::Command;

#[derive(Debug, Default)]
pub(crate) struct ContainerScreen {
    pub(crate) content: String,
}

impl ContainerScreen {
    pub(crate) fn fetch() -> Self {
        let output = Command::new("container").arg("list").output();

        let content = match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if !stdout.is_empty() { stdout } else { stderr }
            }
            Err(e) => format!("Failed to run container list: {e}"),
        };

        ContainerScreen { content }
    }
}
