use super::*;

fn wallpaper() -> Wallpaper {
    Wallpaper {
        width: 2,
        height: 2,
        // Source alpha is deliberately varied: Lock's wallpaper stays opaque.
        rgba: vec![255, 0, 0, 0, 0, 255, 0, 128, 0, 0, 255, 255, 255, 255, 255, 64],
    }
}

#[test]
fn prepared_cover_preserves_center_crop_and_opaque_pixels() {
    let wallpaper = wallpaper();
    for (width, height, expected) in [
        (2, 2, vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff]),
        (4, 2, vec![0xffff_0000, 0xffff_0000, 0xff00_ff00, 0xff00_ff00,
                    0xff00_00ff, 0xff00_00ff, 0xffff_ffff, 0xffff_ffff]),
        (1, 2, vec![0xffff_0000, 0xff00_00ff]),
    ] {
        let mut canvas = Canvas::try_new(width, height, 0).unwrap();
        let mut cache = None;
        wallpaper.cover_cached(&mut canvas, &mut cache);
        assert_eq!(canvas.pixels(), expected);
        assert_eq!(cache.unwrap().pixels(), expected);
    }
}

#[test]
fn changing_dim_does_not_modify_or_reallocate_the_prepared_image() {
    let wallpaper = wallpaper();
    let mut canvas = Canvas::try_new(2, 2, 0).unwrap();
    let mut cache = None;
    wallpaper.cover_cached(&mut canvas, &mut cache);
    let allocation = cache.as_ref().unwrap().pixels().as_ptr();

    for dim in [0, 128, 255, 0] {
        wallpaper.cover_cached(&mut canvas, &mut cache);
        tint(&mut canvas, Rgb::BLACK, dim);
        let expected = Rgb::BLACK.blend(Rgb::new(255, 0, 0), dim).argb(0xff);
        assert_eq!(canvas.pixels()[0], expected);
        assert_eq!(cache.as_ref().unwrap().pixels()[0], 0xffff_0000);
        assert_eq!(cache.as_ref().unwrap().pixels().as_ptr(), allocation);
    }
}

#[test]
fn outputs_keep_separate_caches_and_resizing_replaces_only_one() {
    let wallpaper = wallpaper();
    let mut first = Canvas::try_new(2, 2, 0).unwrap();
    let mut second = Canvas::try_new(4, 2, 0).unwrap();
    let (mut first_cache, mut second_cache) = (None, None);
    wallpaper.cover_cached(&mut first, &mut first_cache);
    wallpaper.cover_cached(&mut second, &mut second_cache);
    let second_allocation = second_cache.as_ref().unwrap().pixels().as_ptr();

    first = Canvas::try_new(1, 2, 0).unwrap();
    wallpaper.cover_cached(&mut first, &mut first_cache);
    wallpaper.cover_cached(&mut second, &mut second_cache);
    assert_eq!(first.pixels(), [0xffff_0000, 0xff00_00ff]);
    assert_eq!(first_cache.as_ref().unwrap().width(), 1);
    assert_eq!(second_cache.as_ref().unwrap().pixels().as_ptr(), second_allocation);
    assert_eq!(second_cache.as_ref().unwrap().width(), 4);
}

#[test]
fn empty_wallpaper_preserves_the_existing_background() {
    let wallpaper = Wallpaper { width: 0, height: 0, rgba: Vec::new() };
    let mut canvas = Canvas::try_new(2, 2, 0xff12_3456).unwrap();
    let mut cache = None;
    wallpaper.cover_cached(&mut canvas, &mut cache);
    assert_eq!(canvas.pixels(), [0xff12_3456; 4]);
    assert!(cache.is_none());
}

#[test]
fn animated_composition_and_zoom_changes_leave_the_cache_pristine() {
    let wallpaper = wallpaper();
    let mut design = rsdm_ui::Design::from_config(&rsdm_core::domain::DesignConfig::default());
    design.background = rsdm_core::domain::Background::Plasma;
    let font = crate::render::Font::default();
    let mut cache = None;
    let mut allocation = None;
    for (zoom, frame, dim) in [(1, 0, 10), (2, 7, 0), (1, 42, 6)] {
        design.dim = dim;
        let mut canvas = Canvas::try_new(64, 64, 0).unwrap();
        assert!(crate::compose_lock_base(
            &mut canvas, &design, Some(&wallpaper), &font, zoom, frame, &mut cache,
        ));
        let cached = cache.as_ref().unwrap();
        let address = cached.pixels().as_ptr();
        assert_eq!(*allocation.get_or_insert(address), address);
        assert_eq!(cached.pixels()[0], 0xffff_0000);
        assert_eq!(*cached.pixels().last().unwrap(), 0xffff_ffff);
        assert!(canvas.pixels().iter().all(|pixel| pixel >> 24 == 255));
    }
}
