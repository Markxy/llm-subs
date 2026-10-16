pub mod folders;
pub mod jobs;
pub mod models;
pub mod setup;
pub mod translation;
pub mod usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Jobs,
    Folders,
    Translation,
    Models,
    Usage,
    Setup,
}

impl View {
    pub const ALL: [View; 6] = [View::Jobs, View::Folders, View::Translation, View::Models, View::Usage, View::Setup];

    pub fn label(self) -> &'static str {
        match self {
            View::Jobs => "Jobs",
            View::Folders => "Watched folders",
            View::Translation => "Translation",
            View::Models => "Models",
            View::Usage => "Usage",
            View::Setup => "Setup",
        }
    }
}
