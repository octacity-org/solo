//! CSS generic font families as concrete family names.
//!
//! GPUI's Linux text system loads only faces whose family name equals the
//! requested name, so `fontFamily: "monospace"` loads nothing and the text
//! falls back to GPUI's own font stack. On Linux, Solo asks fontconfig,
//! which is what the user's desktop configures, as `fc-match` does, and
//! falls back to the theme's defaults. Other platforms get the name
//! unchanged.

use gpui::SharedString;

/// The family GPUI should load for a `fontFamily` style value.
pub(crate) fn resolve(family: &str) -> SharedString {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Some(resolved) = linux::generic(family) {
        return resolved;
    }
    SharedString::from(family.to_owned())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use gpui::SharedString;
    use std::ffi::{CStr, CString};
    use std::ptr;
    use std::sync::OnceLock;

    static MONOSPACE: OnceLock<SharedString> = OnceLock::new();
    static SANS_SERIF: OnceLock<SharedString> = OnceLock::new();
    static SERIF: OnceLock<SharedString> = OnceLock::new();
    static SYSTEM_UI: OnceLock<SharedString> = OnceLock::new();

    /// The concrete family for a generic name, looked up once per name.
    /// `None` for any other name.
    pub(super) fn generic(family: &str) -> Option<SharedString> {
        let (resolved, fallback) = match family {
            "monospace" => (&MONOSPACE, crate::theme::system_mono()),
            "sans-serif" => (&SANS_SERIF, crate::theme::system_sans()),
            "serif" => (&SERIF, "DejaVu Serif"),
            "system-ui" => (&SYSTEM_UI, crate::theme::system_sans()),
            _ => return None,
        };
        let resolved = resolved.get_or_init(|| {
            fontconfig_family(family)
                .unwrap_or_else(|| fallback.to_owned())
                .into()
        });
        Some(resolved.clone())
    }

    /// The family of fontconfig's best match for `name`.
    fn fontconfig_family(name: &str) -> Option<String> {
        use fontconfig_sys::constants::FC_FAMILY;
        use fontconfig_sys::*;

        let name = CString::new(name).ok()?;
        // SAFETY: each pattern is checked for null before use and destroyed
        // once; the family string is copied before its pattern is destroyed.
        // A null config is fontconfig's current configuration.
        unsafe {
            let pattern = FcNameParse(name.as_ptr().cast());
            if pattern.is_null() {
                return None;
            }
            FcConfigSubstitute(ptr::null_mut(), pattern, FcMatchPattern);
            FcDefaultSubstitute(pattern);
            let mut result = FcResultNoMatch;
            let matched = FcFontMatch(ptr::null_mut(), pattern, &mut result);
            FcPatternDestroy(pattern);
            if matched.is_null() {
                return None;
            }
            let mut value: *mut FcChar8 = ptr::null_mut();
            let family = (FcPatternGetString(matched, FC_FAMILY.as_ptr(), 0, &mut value)
                == FcResultMatch
                && !value.is_null())
            .then(|| CStr::from_ptr(value.cast()).to_string_lossy().into_owned());
            FcPatternDestroy(matched);
            family.filter(|family| !family.is_empty())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn resolves_generic_names_to_installed_families() {
            for name in ["monospace", "sans-serif", "serif", "system-ui"] {
                let family = generic(name).expect("a generic name");
                assert!(!family.is_empty());
                assert_ne!(family.as_ref(), name);
            }
        }

        #[test]
        fn leaves_concrete_names_alone() {
            assert_eq!(generic("DejaVu Sans Mono"), None);
            assert_eq!(super::super::resolve("Inter").as_ref(), "Inter");
        }
    }
}
