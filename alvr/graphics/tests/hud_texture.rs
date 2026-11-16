use alvr_common::glam::Vec3;
use alvr_graphics::{LobbyButton, build_hud_pixels, lobby_ray_hit};

#[test]
fn pointer_hits_the_button_under_it() {
    // The launch button sits left of center, a bit below eye height, on the wall 5 m ahead
    let button_center = Vec3::new(-0.7525, 1.33, -5.0);
    let controller = Vec3::new(0.2, 1.5, -0.5);
    let direction = (button_center - controller).normalize();

    let (hit, button) = lobby_ray_hit(controller, direction).unwrap();
    assert!(hit.distance(button_center) < 0.01, "hit at {hit:?}");
    assert_eq!(button, Some(LobbyButton::LaunchSteamvr));

    // Between the buttons the wall still blocks the beam, but no button is selected
    let gap = Vec3::new(0.0, 1.33, -5.0);
    let (_, button) = lobby_ray_hit(controller, (gap - controller).normalize()).unwrap();
    assert_eq!(button, None);

    // Pointing away from every wall misses entirely
    assert!(lobby_ray_hit(controller, Vec3::Y).is_none());
}

#[test]
fn render_hud_texture_preview() {
    let message = "ALVR v21.0.0-dev14\nhostname: 6046.client.local\nIP: fe80::c815:be7d:f4db:a6e0\nUSB network: 5.1 Gbps\n\nSearching for streamer...\nOpen ALVR on your PC then click \"Trust\"\nnext to the device entry";

    let pixels = build_hud_pixels(message, Some(LobbyButton::LaunchSteamvr), None);
    let hud = image::RgbaImage::from_raw(1024, 1024, pixels).unwrap();

    // The layer is premultiplied over a black background, so raw RGB is what the headset shows
    let mut preview = image::RgbImage::new(1024, 1024);
    for (x, y, pixel) in hud.enumerate_pixels() {
        preview.put_pixel(x, y, image::Rgb([pixel[0], pixel[1], pixel[2]]));
    }

    let path = std::env::temp_dir().join("alvr_hud_preview.png");
    preview.save(&path).unwrap();

    println!("saved {}", path.display());

    // ASCII dump of the first message line, alpha channel of the premultiplied buffer
    for y in 128..170 {
        let mut row = String::new();
        for x in 250..410 {
            let alpha = hud.get_pixel(x, y)[3];
            row.push(match alpha {
                0 => '.',
                1..=64 => ':',
                65..=160 => '+',
                _ => '#',
            });
        }
        println!("{y} {row}");
    }
}
