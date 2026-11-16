use crate::{
    graphics::{self, ProjectionLayerAlphaConfig, ProjectionLayerBuilder},
    interaction::{self, InteractionContext},
};
use alvr_client_core::lobby_command::{self, LobbyCommand};
use alvr_common::{
    Pose, ViewParams,
    glam::{UVec2, Vec3},
    inputs::{LEFT_TRIGGER_CLICK_ID, RIGHT_TRIGGER_CLICK_ID},
    parking_lot::RwLock,
};
use alvr_graphics::{
    GraphicsContext, LobbyButton, LobbyLaser, LobbyRenderer, LobbyViewParams, SDR_FORMAT_GL,
    lobby_ray_hit,
};
use alvr_system_info::Platform;
use openxr as xr;
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const PRESSED_HIGHLIGHT_DURATION: Duration = Duration::from_millis(900);
const LASER_LENGTH: f32 = 6.0;

pub struct Lobby {
    xr_session: xr::Session<xr::OpenGlEs>,
    interaction_ctx: Arc<RwLock<InteractionContext>>,
    platform: Platform,
    reference_space: xr::Space,
    swapchains: [xr::Swapchain<xr::OpenGlEs>; 2],
    view_resolution: UVec2,
    reference_space_type: xr::ReferenceSpaceType,
    renderer: LobbyRenderer,
    hud_message: String,
    hovered_button: Option<LobbyButton>,
    pressed_button: Option<(LobbyButton, Instant)>,
}

impl Lobby {
    pub fn new(
        xr_session: xr::Session<xr::OpenGlEs>,
        gfx_ctx: Rc<GraphicsContext>,
        interaction_ctx: Arc<RwLock<InteractionContext>>,
        platform: Platform,
        view_resolution: UVec2,
        initial_hud_message: &str,
    ) -> Self {
        let reference_space_type = if xr_session.instance().exts().ext_local_floor.is_some() {
            xr::ReferenceSpaceType::LOCAL_FLOOR_EXT
        } else {
            // The Quest 1 doesn't support LOCAL_FLOOR_EXT, recentering is required for AppLab, but
            // the Quest 1 is excluded from AppLab anyway.
            xr::ReferenceSpaceType::STAGE
        };

        let reference_space = interaction::get_reference_space(&xr_session, reference_space_type);

        let swapchains = [
            graphics::create_swapchain(&xr_session, &gfx_ctx, view_resolution, SDR_FORMAT_GL, None),
            graphics::create_swapchain(&xr_session, &gfx_ctx, view_resolution, SDR_FORMAT_GL, None),
        ];

        let renderer = LobbyRenderer::new(
            gfx_ctx,
            view_resolution,
            [
                swapchains[0]
                    .enumerate_images()
                    .unwrap()
                    .iter()
                    .map(|i| *i as _)
                    .collect(),
                swapchains[1]
                    .enumerate_images()
                    .unwrap()
                    .iter()
                    .map(|i| *i as _)
                    .collect(),
            ],
            initial_hud_message,
        );

        Self {
            xr_session,
            interaction_ctx,
            platform,
            reference_space,
            swapchains,
            view_resolution,
            reference_space_type,
            renderer,
            hud_message: initial_hud_message.to_owned(),
            hovered_button: None,
            pressed_button: None,
        }
    }

    pub fn update_reference_space(&mut self) {
        self.reference_space =
            interaction::get_reference_space(&self.xr_session, self.reference_space_type);
    }

    pub fn update_hud_message(&mut self, message: &str) {
        self.hud_message = message.to_owned();
        self.refresh_hud();
    }

    fn refresh_hud(&mut self) {
        let pressed_button = self.pressed_button.and_then(|(button, pressed_at)| {
            (pressed_at.elapsed() < PRESSED_HIGHLIGHT_DURATION).then_some(button)
        });

        self.renderer
            .update_hud(&self.hud_message, self.hovered_button, pressed_button);
    }

    pub fn render(&mut self, vsync_time: Duration) -> ProjectionLayerBuilder<'_> {
        let xr_vsync_time = crate::to_xr_time(vsync_time);

        let (flags, maybe_views) = self
            .xr_session
            .locate_views(
                xr::ViewConfigurationType::PRIMARY_STEREO,
                xr_vsync_time,
                &self.reference_space,
            )
            .unwrap();

        let views = if flags.contains(xr::ViewStateFlags::ORIENTATION_VALID) {
            maybe_views
        } else {
            vec![crate::default_view(), crate::default_view()]
        };

        self.xr_session
            .sync_actions(&[(&self.interaction_ctx.read().action_set).into()])
            .ok();

        // future_time doesn't have to be any particular value, just something after vsync_time
        let future_time = vsync_time + Duration::from_millis(80);
        let left_hand_data = interaction::get_hand_data(
            &self.xr_session,
            self.platform,
            &self.reference_space,
            vsync_time,
            future_time,
            &self.interaction_ctx.read().hands_interaction[0],
            &mut Pose::default(),
            &mut Pose::default(),
        );
        let right_hand_data = interaction::get_hand_data(
            &self.xr_session,
            self.platform,
            &self.reference_space,
            vsync_time,
            future_time,
            &self.interaction_ctx.read().hands_interaction[1],
            &mut Pose::default(),
            &mut Pose::default(),
        );

        let mut additional_motions = vec![];
        if let Some(source) = &self.interaction_ctx.read().body_source {
            additional_motions.extend(
                interaction::get_bd_motion_trackers(source, vsync_time)
                    .iter()
                    .map(|(_, motion)| *motion),
            )
        }

        let body_skeleton = self
            .interaction_ctx
            .read()
            .body_source
            .as_ref()
            .and_then(|source| {
                interaction::get_body_skeleton(source, &self.reference_space, vsync_time)
            });

        // Each controller (or tracked hand) casts a laser at the text wall. Without tracked hands
        // the gaze ray keeps the buttons usable.
        let mut lasers = Vec::with_capacity(2);
        let mut hand_hits = [None; 2];
        let mut hovered_button = None;

        for (index, data) in [&left_hand_data, &right_hand_data].iter().enumerate() {
            let Some(motion) = data.aim_motion.or(data.grip_motion) else {
                continue;
            };

            let start = motion.pose.position;
            let direction = motion.pose.orientation * Vec3::NEG_Z;
            let (end, button) = match lobby_ray_hit(start, direction) {
                Some((hit, button)) => (hit, button),
                None => (start + direction * LASER_LENGTH, None),
            };

            lasers.push(LobbyLaser { start, end });
            hand_hits[index] = button;
            hovered_button = hovered_button.or(button);
        }

        if lasers.is_empty() {
            let left_eye = crate::from_xr_pose(views[0].pose);
            let right_eye = crate::from_xr_pose(views[1].pose);
            let gaze_origin = (left_eye.position + right_eye.position) * 0.5;
            let gaze_direction = left_eye.orientation * Vec3::NEG_Z;

            hovered_button =
                lobby_ray_hit(gaze_origin, gaze_direction).and_then(|(_, button)| button);
        }

        if hovered_button != self.hovered_button {
            self.hovered_button = hovered_button;
            self.refresh_hud();
        }

        if let Some((_, pressed_at)) = self.pressed_button
            && pressed_at.elapsed() >= PRESSED_HIGHLIGHT_DURATION
        {
            self.pressed_button = None;
            self.refresh_hud();
        }

        let trigger_clicked = {
            let interaction_context = self.interaction_ctx.read();
            [&*LEFT_TRIGGER_CLICK_ID, &*RIGHT_TRIGGER_CLICK_ID].map(|id| {
                match interaction_context.button_actions.get(id) {
                    Some(interaction::ButtonAction::Binary(action)) => action
                        .state(&self.xr_session, xr::Path::NULL)
                        .map(|state| state.changed_since_last_sync && state.current_state)
                        .unwrap_or(false),
                    _ => false,
                }
            })
        };

        for (index, clicked) in trigger_clicked.into_iter().enumerate() {
            if !clicked {
                continue;
            }

            if let Some(button) = hand_hits[index].or(self.hovered_button) {
                lobby_command::request_steamvr(match button {
                    LobbyButton::LaunchSteamvr => LobbyCommand::LaunchSteamvr,
                    LobbyButton::RestartSteamvr => LobbyCommand::RestartSteamvr,
                });

                self.pressed_button = Some((button, Instant::now()));
                self.refresh_hud();

                break;
            }
        }

        let left_swapchain_idx = self.swapchains[0].acquire_image().unwrap();
        let right_swapchain_idx = self.swapchains[1].acquire_image().unwrap();

        self.swapchains[0]
            .wait_image(xr::Duration::INFINITE)
            .unwrap();
        self.swapchains[1]
            .wait_image(xr::Duration::INFINITE)
            .unwrap();

        self.renderer.render(
            [
                LobbyViewParams {
                    view_params: ViewParams {
                        pose: crate::from_xr_pose(views[0].pose),
                        fov: crate::from_xr_fov(views[0].fov),
                    },
                    swapchain_index: left_swapchain_idx,
                },
                LobbyViewParams {
                    view_params: ViewParams {
                        pose: crate::from_xr_pose(views[1].pose),
                        fov: crate::from_xr_fov(views[1].fov),
                    },
                    swapchain_index: right_swapchain_idx,
                },
            ],
            [left_hand_data, right_hand_data],
            body_skeleton,
            additional_motions,
            &lasers,
            false,
            cfg!(debug_assertions),
        );

        self.swapchains[0].release_image().unwrap();
        self.swapchains[1].release_image().unwrap();

        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: self.view_resolution.x as _,
                height: self.view_resolution.y as _,
            },
        };

        ProjectionLayerBuilder::new(
            &self.reference_space,
            [
                xr::CompositionLayerProjectionView::new()
                    .pose(views[0].pose)
                    .fov(views[0].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&self.swapchains[0])
                            .image_array_index(0)
                            .image_rect(rect),
                    ),
                xr::CompositionLayerProjectionView::new()
                    .pose(views[1].pose)
                    .fov(views[1].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&self.swapchains[1])
                            .image_array_index(0)
                            .image_rect(rect),
                    ),
            ],
            Some(ProjectionLayerAlphaConfig {
                premultiplied: true,
            }),
            None,
        )
    }
}
