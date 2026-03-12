use std::process::Command;

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub(crate) enum ContainerStatus {
    Running,
    #[default]
    NotRunning,
}

#[derive(Debug, Default)]
pub(crate) struct StatusScreen {
    pub(crate) status: ContainerStatus,
    pub(crate) message: String,
}

impl StatusScreen {
    pub(crate) fn fetch() -> Self {
        let output = Command::new("container")
            .args(["system", "status"])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let message = if !stdout.is_empty() { stdout } else { stderr };
                let status = if out.status.success() {
                    ContainerStatus::Running
                } else {
                    ContainerStatus::NotRunning
                };
                StatusScreen { status, message }
            }
            Err(e) => StatusScreen {
                status: ContainerStatus::NotRunning,
                message: format!("Failed to run container: {e}"),
            },
        }
    }
}
