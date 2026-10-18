//! The icons camctl uses on top of gpui-component's defaults. Only the
//! listed SVGs are compiled in.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    CamctlIcons,
    [Camera, LocateFixed, Moon, Move, Sun, Video, ZoomIn]
);

/// camctl's icons first, then gpui-component's own.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = CamctlIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(CamctlIcons.list(path)?);
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_camctl_draws_is_bundled() {
        use gpui_kit::assets::IconName;
        for icon in [
            IconName::Camera,
            IconName::LocateFixed,
            IconName::Moon,
            IconName::Move,
            IconName::Sun,
            IconName::Video,
            IconName::ZoomIn,
        ] {
            let path = icon.path();
            assert!(AppAssets.load(&path).unwrap().is_some(), "{path} missing");
        }
    }
}
