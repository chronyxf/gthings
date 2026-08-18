use crate::error::Result;
use crate::session::Session;
use crate::tab::Tab;

impl Session {
    /// Create a background tab (no window focus steal) at `about:blank`.
    ///
    /// Background tabs are isolated from foreground tabs and do not block
    /// concurrent CDP operations.
    pub async fn create_background_tab(&self) -> Result<Tab> {
        Tab::create_background(self).await
    }

    /// Close a tab
    pub async fn close_tab(&self, tab: Tab) -> Result<()> {
        tab.close(self).await
    }
}
