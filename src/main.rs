//! Examples for using 2D rendering.
//!
//! On the left is a 2D view, on the right a 3D view of the same scene.

use std::borrow::Cow;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crabgrab::prelude::{CapturableContent, CapturableContentFilter, CaptureConfig, CapturePixelFormat, CaptureStream, FrameBitmap, FrameBitmapBgraUnorm8x4, MetalVideoFrameExt, MetalVideoFramePlaneTexture, StreamEvent, VideoFrameBitmap, WgpuCaptureConfigExt, WgpuVideoFrameExt, WgpuVideoFramePlaneTexture};
use itertools::Itertools as _;
use re_renderer::Hsva;

#[allow(unused)]
struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl AsRef<wgpu::Device> for Gfx {
    fn as_ref(&self) -> &wgpu::Device {
        &self.device
    }
}

use re_renderer::{
    renderer::{
        ColormappedTexture, LineStripFlags, RectangleDrawData, RectangleOptions, TextureFilterMag,
        TextureFilterMin, TexturedRect,
    },
    resource_managers::{GpuTexture2D, Texture2DCreationDesc},
    view_builder::{self, Projection, TargetConfiguration, ViewBuilder},
    Color32, LineDrawableBuilder, PointCloudBuilder, Size,
};
use wgpu::Texture;
use once_cell::sync::Lazy;

mod framework;

struct Frame {
    frame_texture: metal::Texture,
    frame_id: u64,
}

static SCREEN_TEXTURE: Lazy<Arc<Mutex<Option<Frame>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));

struct Render2D {
    rerun_logo_texture: GpuTexture2D,
    rerun_logo_texture_width: u32,
    rerun_logo_texture_height: u32,
}

impl framework::Example for Render2D {
    fn title() -> &'static str {
        "2D Rendering"
    }

    fn new(re_ctx: &re_renderer::RenderContext) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();

        runtime.spawn(async {
            let token = match CaptureStream::test_access(false) {
                Some(token) => token,
                None => CaptureStream::request_access(false).await.expect("Expected capture access")
            };

            let wgpu_instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                #[cfg(target_os = "windows")]
                backends: wgpu::Backends::DX12,
                #[cfg(target_os = "macos")]
                backends: wgpu::Backends::METAL,
                flags: wgpu::InstanceFlags::default(),
                dx12_shader_compiler: wgpu::Dx12Compiler::default(),
                gles_minor_version: wgpu::Gles3MinorVersion::default(),
            });
            let wgpu_adapter = wgpu_instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                force_fallback_adapter: false,
                compatible_surface: None,
            }).await.expect("Expected wgpu adapter");
            let (wgpu_device, wgpu_queue) = wgpu_adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("wgpu adapter"),
                required_features: wgpu::Features::default(),
                required_limits: wgpu::Limits::default(),
            }, None).await.expect("Expected wgpu device");
            let gfx = Arc::new(Gfx {
                device: wgpu_device,
                queue: wgpu_queue,
            });

            let filter = CapturableContentFilter { windows: None, displays: true };
            let content = CapturableContent::new(filter).await.unwrap();
            let display = content.displays().next()
                .expect("Expected at least one capturable display");
            let config = CaptureConfig::with_display(display, CapturePixelFormat::Bgra8888)
                .with_wgpu_device(gfx.clone())
                .expect("Expected config with wgpu device");

            let mut stream = CaptureStream::new(token, config, |result| {
                println!("result: {:?}", result);
                if let Ok(StreamEvent::Video(frame)) = result {
                    let frame_id = frame.frame_id();

                    match frame.get_metal_texture(MetalVideoFramePlaneTexture::Rgba) {
                        Ok(texture) => {
                            SCREEN_TEXTURE.lock().unwrap().replace(Frame {
                                frame_texture: texture,
                                frame_id,
                            });
                        }
                        Err(e) => {
                            println!("Bitmap error: {:?}", e);
                        }
                    }
                }
            }).unwrap();
            let _ = ManuallyDrop::new(stream);

            // tokio::time::sleep(Duration::from_millis(20000000)).await;
            //
            // stream.stop().unwrap();
        });
        let _ = ManuallyDrop::new(runtime);

        let rerun_logo =
            image::load_from_memory(include_bytes!("logo_dark_mode.png")).unwrap();

        let image_data = rerun_logo.as_rgba8().unwrap().to_vec();

        let rerun_logo_texture = re_ctx
            .texture_manager_2d
            .create(
                &re_ctx.gpu_resources.textures,
                &Texture2DCreationDesc {
                    label: "rerun logo".into(),
                    data: image_data.into(),
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    width: rerun_logo.width(),
                    height: rerun_logo.height(),
                },
            )
            .expect("Failed to create texture for rerun logo");
        Render2D {
            rerun_logo_texture,

            rerun_logo_texture_width: rerun_logo.width(),
            rerun_logo_texture_height: rerun_logo.height(),
        }
    }

    fn draw(
        &mut self,
        re_ctx: &re_renderer::RenderContext,
        resolution: [u32; 2],
        time: &framework::Time,
        pixels_from_point: f32,
    ) -> Vec<framework::ViewDrawResult> {
        puffin::GlobalProfiler::lock().new_frame();
        puffin::profile_function!();
        let splits = framework::split_resolution(resolution, 1, 2).collect::<Vec<_>>();

        let screen_size = glam::vec2(
            splits[0].resolution_in_pixel[0] as f32,
            splits[0].resolution_in_pixel[1] as f32,
        );

        let mut line_strip_builder = LineDrawableBuilder::new(re_ctx);
        line_strip_builder.reserve_strips(128).unwrap();
        line_strip_builder.reserve_vertices(2048).unwrap();

        // Blue rect outline around the bottom right quarter.
        {
            let mut line_batch = line_strip_builder.batch("quads");
            let line_radius = 10.0;
            let blue_rect_position = screen_size * 0.5 - glam::vec2(line_radius, line_radius);
            line_batch
                .add_rectangle_outline_2d(
                    blue_rect_position,
                    glam::vec2(screen_size.x * 0.5, 0.0),
                    glam::vec2(0.0, screen_size.y * 0.5),
                )
                .radius(Size::new_scene(line_radius))
                .color(Color32::BLUE);

            // .. within, a orange rectangle
            line_batch
                .add_rectangle_outline_2d(
                    blue_rect_position + screen_size * 0.125,
                    glam::vec2(screen_size.x * 0.25, 0.0),
                    glam::vec2(0.0, screen_size.y * 0.25),
                )
                .radius(Size::new_scene(5.0))
                .color(Color32::from_rgb(255, 100, 1));
        }

        // All variations of line caps
        {
            let mut line_batch = line_strip_builder.batch("line cap variations");
            for (i, flags) in [
                LineStripFlags::empty(),
                LineStripFlags::FLAG_CAP_START_ROUND,
                LineStripFlags::FLAG_CAP_END_ROUND,
                LineStripFlags::FLAG_CAP_START_TRIANGLE,
                LineStripFlags::FLAG_CAP_END_TRIANGLE,
                LineStripFlags::FLAG_CAP_START_ROUND | LineStripFlags::FLAG_CAP_END_ROUND,
                LineStripFlags::FLAG_CAP_START_ROUND | LineStripFlags::FLAG_CAP_END_TRIANGLE,
                LineStripFlags::FLAG_CAP_START_TRIANGLE | LineStripFlags::FLAG_CAP_END_ROUND,
                LineStripFlags::FLAG_CAP_START_TRIANGLE | LineStripFlags::FLAG_CAP_END_TRIANGLE,
            ]
                .iter()
                .enumerate()
            {
                let y = (i + 1) as f32 * 70.0;
                line_batch
                    .add_segment_2d(glam::vec2(70.0, y), glam::vec2(400.0, y))
                    .radius(Size::new_scene(15.0))
                    .flags(*flags | LineStripFlags::FLAG_COLOR_GRADIENT);
            }
        }

        // Lines with non-default arrow heads - long thin arrows.
        {
            let mut line_batch = line_strip_builder
                .batch("larger arrowheads")
                .triangle_cap_length_factor(15.0)
                .triangle_cap_width_factor(3.0);
            for (i, flags) in [
                LineStripFlags::FLAG_CAP_START_TRIANGLE | LineStripFlags::FLAG_CAP_END_ROUND,
                LineStripFlags::FLAG_CAP_START_ROUND | LineStripFlags::FLAG_CAP_END_TRIANGLE,
                LineStripFlags::FLAG_CAP_START_TRIANGLE | LineStripFlags::FLAG_CAP_END_TRIANGLE,
            ]
                .iter()
                .enumerate()
            {
                let y = (i + 1) as f32 * 40.0 + 650.0;
                line_batch
                    .add_segment_2d(glam::vec2(70.0, y), glam::vec2(400.0, y))
                    .radius(Size::new_scene(5.0))
                    .flags(*flags);
            }
        }

        // Lines with different kinds of radius
        // The first two lines are the same thickness if there no (!) scaling.
        // Moving the windows to a high dpi screen makes the second one bigger.
        // Also, it looks different under perspective projection.
        // The third line is automatic thickness which is determined by the line renderer implementation.
        {
            let mut line_batch = line_strip_builder.batch("radius variations");
            line_batch
                .add_segment_2d(glam::vec2(500.0, 10.0), glam::vec2(1000.0, 10.0))
                .radius(Size::new_scene(4.0))
                .color(Color32::from_rgb(255, 180, 1));
            line_batch
                .add_segment_2d(glam::vec2(500.0, 30.0), glam::vec2(1000.0, 30.0))
                .radius(Size::new_points(4.0))
                .color(Color32::from_rgb(255, 180, 1));
            line_batch
                .add_segment_2d(glam::vec2(500.0, 60.0), glam::vec2(1000.0, 60.0))
                .radius(Size::AUTO)
                .color(Color32::from_rgb(255, 180, 1));
            line_batch
                .add_segment_2d(glam::vec2(500.0, 90.0), glam::vec2(1000.0, 90.0))
                .radius(Size::AUTO_LARGE)
                .color(Color32::from_rgb(255, 180, 1));
        }

        // Points with different kinds of radius
        // The first two points are the same thickness if there no (!) scaling.
        // Moving the windows to a high dpi screen makes the second one bigger.
        // Also, it looks different under perspective projection.
        // The third point is automatic thickness which is determined by the point renderer implementation.
        let mut point_cloud_builder = PointCloudBuilder::new(re_ctx);
        point_cloud_builder.reserve(128).unwrap();
        point_cloud_builder.batch("points").add_points_2d(
            &[
                glam::vec3(500.0, 120.0, 0.0),
                glam::vec3(520.0, 120.0, 0.0),
                glam::vec3(540.0, 120.0, 0.0),
                glam::vec3(560.0, 120.0, 0.0),
            ],
            &[
                Size::new_scene(4.0),
                Size::new_points(4.0),
                Size::AUTO,
                Size::AUTO_LARGE,
            ],
            &[Color32::from_rgb(55, 180, 1); 4],
            &[re_renderer::PickingLayerInstanceId::default(); 4],
        );

        // Pile stuff to test for overlap handling.
        // Do in individual batches to test depth offset.
        {
            let num_lines = 20_i16;
            let y_range = 800.0..880.0;

            // Cycle through which line is on top.
            let top_line = ((time.seconds_since_startup() * 6.0) as i16 % (num_lines * 2 - 1)
                - num_lines)
                .abs();
            for i in 0..num_lines {
                let depth_offset = if i < top_line { i } else { top_line * 2 - i };
                let mut batch = line_strip_builder
                    .batch(format!("overlapping objects {i}"))
                    .depth_offset(depth_offset);

                let x = 15.0 * i as f32 + 20.0;
                batch
                    .add_segment_2d(glam::vec2(x, y_range.start), glam::vec2(x, y_range.end))
                    .color(Hsva::new(0.25 / num_lines as f32 * i as f32, 1.0, 0.5, 1.0).into())
                    .radius(Size::new_points(10.0))
                    .flags(LineStripFlags::FLAG_COLOR_GRADIENT);
            }

            let num_points = 8;
            let size = Size::new_points(3.0);

            let positions = (0..num_points)
                .map(|i| {
                    glam::vec3(
                        30.0 * i as f32 + 20.0,
                        y_range.start
                            + (y_range.end - y_range.start) / num_points as f32 * i as f32,
                        0.0,
                    )
                })
                .collect_vec();

            let sizes = vec![size; num_points];

            let colors = vec![Color32::WHITE; num_points];

            let picking_ids = vec![re_renderer::PickingLayerInstanceId::default(); num_points];

            point_cloud_builder
                .batch("points overlapping with lines")
                .depth_offset(5)
                .add_points_2d(&positions, &sizes, &colors, &picking_ids);
        }

        let line_strip_draw_data = line_strip_builder.into_draw_data().unwrap();
        let point_draw_data = point_cloud_builder.into_draw_data().unwrap();

        let image_scale = 4.0;

        let texture = if let Some(texture) = SCREEN_TEXTURE.lock().unwrap().as_ref() {
            puffin::profile_scope!("screen texture");
            let Frame { frame_texture, .. } = texture;
            let screen_texture = re_ctx.texture_manager_2d.create_from_metal_texture(
                "screen texture",
                &re_ctx.gpu_resources.textures,
                frame_texture.clone(),
            ).unwrap();

            screen_texture
        } else {
            self.rerun_logo_texture.clone()
        };


        let rectangle_draw_data = RectangleDrawData::new(
            re_ctx,
            &[
                TexturedRect {
                    top_left_corner_position: glam::vec3(500.0, 120.0, -0.05),
                    extent_u: self.rerun_logo_texture_width as f32 * image_scale * glam::Vec3::X,
                    extent_v: self.rerun_logo_texture_height as f32 * image_scale * glam::Vec3::Y,
                    colormapped_texture: ColormappedTexture::from_unorm_rgba(
                        texture
                    ),
                    options: RectangleOptions {
                        texture_filter_magnification: TextureFilterMag::Nearest,
                        texture_filter_minification: TextureFilterMin::Linear,
                        ..Default::default()
                    },
                },
            ],
        )
            .unwrap();

        vec![
            // 2D view to the left
            {
                let mut view_builder = ViewBuilder::new(
                    re_ctx,
                    TargetConfiguration {
                        name: "2D".into(),
                        resolution_in_pixel: splits[0].resolution_in_pixel,
                        view_from_world: macaw::IsoTransform::IDENTITY,
                        projection_from_view: Projection::Orthographic {
                            camera_mode:
                            view_builder::OrthographicCameraMode::TopLeftCornerAndExtendZ,
                            vertical_world_size: splits[0].resolution_in_pixel[1] as f32,
                            far_plane_distance: 1000.0,
                        },
                        pixels_from_point,
                        ..Default::default()
                    },
                );
                view_builder.queue_draw(line_strip_draw_data.clone());
                view_builder.queue_draw(point_draw_data.clone());
                view_builder.queue_draw(rectangle_draw_data.clone());
                let command_buffer = view_builder
                    .draw(re_ctx, re_renderer::Rgba::TRANSPARENT)
                    .unwrap();
                framework::ViewDrawResult {
                    view_builder,
                    command_buffer,
                    target_location: splits[0].target_location,
                }
            },
            // and 3D view of the same scene to the right
            {
                let seconds_since_startup = time.seconds_since_startup();
                let camera_rotation_center = screen_size.extend(0.0) * 0.5;
                let camera_position = glam::vec3(
                    seconds_since_startup.sin(),
                    0.5,
                    seconds_since_startup.cos(),
                ) * screen_size.x.max(screen_size.y)
                    + camera_rotation_center;
                let mut view_builder = ViewBuilder::new(
                    re_ctx,
                    view_builder::TargetConfiguration {
                        name: "3D".into(),
                        resolution_in_pixel: splits[1].resolution_in_pixel,
                        view_from_world: macaw::IsoTransform::look_at_rh(
                            camera_position,
                            camera_rotation_center,
                            glam::Vec3::Y,
                        )
                            .unwrap(),
                        projection_from_view: Projection::Perspective {
                            vertical_fov: 70.0 * std::f32::consts::TAU / 360.0,
                            near_plane_distance: 0.01,
                            aspect_ratio: resolution[0] as f32 / resolution[1] as f32,
                        },
                        pixels_from_point,
                        ..Default::default()
                    },
                );
                let command_buffer = view_builder
                    .queue_draw(line_strip_draw_data)
                    .queue_draw(point_draw_data)
                    .queue_draw(rectangle_draw_data)
                    .draw(re_ctx, re_renderer::Rgba::TRANSPARENT)
                    .unwrap();
                framework::ViewDrawResult {
                    view_builder,
                    command_buffer,
                    target_location: splits[1].target_location,
                }
            },
        ]
    }

    fn on_key_event(&mut self, _input: winit::event::KeyEvent) {}
}

fn main() {
    let server_addr = format!("127.0.0.1:{}", 7901);
    let _puffin_server = puffin_http::Server::new(&server_addr).unwrap();
    eprintln!("Run this to view profiling data:  puffin_viewer {server_addr}");
    puffin::set_scopes_on(true);

    framework::start::<Render2D>();
}
