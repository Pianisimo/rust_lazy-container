mod container_screen;
mod status_screen;

pub(crate) use container_screen::ContainerScreen;
pub(crate) use status_screen::{ContainerStatus, StatusScreen};

#[derive(Debug)]
pub(crate) enum Screen {
    Status(StatusScreen),
    Containers(ContainerScreen),
}

impl Default for Screen {
    fn default() -> Self {
        Screen::Status(StatusScreen::fetch())
    }
}
