use super::AboutBranding;

#[test]
fn about_branding_uses_local_warp_name() {
    let branding = AboutBranding::for_version(Some("v0.2026.07.01.00.01.oss_00"));

    assert_eq!(branding.product_name, "Local-Warp");
    assert_eq!(
        branding.tagline,
        "A fork of warp/openwarp supporting Bring Your Own Key (BYOK) and Bring Your Own Provider (BYOP)."
    );
    assert_eq!(branding.copyright, "Copyright 2026 Local-Warp");
}

#[test]
fn about_branding_displays_release_tag() {
    let branding = AboutBranding::for_version(Some("v0.2026.07.01.00.01.oss_00"));

    assert_eq!(branding.version, "v0.2026.07.01.00.01.oss_00");
}

#[test]
fn about_branding_uses_placeholder_when_tag_is_missing() {
    let branding = AboutBranding::for_version(None);

    assert_eq!(branding.version, "v#.##.###");
}
