// todo: add a texture.
// todo: add mesh.
// todo: add model.
// todo: assimp stuff.

use std::{borrow::Cow, iter::once, mem::size_of, time::Instant};

use bytemuck::cast_slice;
use bytemuck_derive::{Pod, Zeroable};
use futures::executor::block_on;
use glam::{Mat4, Quat, Vec3};
use wgpu::{
    Adapter, Backends, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, BufferDescriptor, BufferSize,
    BufferUsages, Color, CommandEncoderDescriptor, CompareFunction, DepthBiasState,
    DepthStencilState, Device, DeviceDescriptor, Extent3d, Face, FragmentState, FrontFace,
    IndexFormat, Instance, InstanceDescriptor, LoadOp, MultisampleState, Operations,
    PipelineLayoutDescriptor, PowerPreference, PresentMode, PrimitiveState, Queue,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, RequestAdapterOptions, ShaderModuleDescriptor, ShaderSource,
    ShaderStages, StencilState, Surface, SurfaceConfiguration, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};
use wgpu_samples::camera::{Camera, CameraDescriptor, GpuCamera};
use winit::{
    dpi::LogicalSize,
    event::{DeviceEvent, ElementState, Event, MouseScrollDelta, VirtualKeyCode, WindowEvent},
    event_loop::EventLoop,
    platform::run_return::EventLoopExtRunReturn,
    window::{CursorGrabMode, Window, WindowBuilder},
};

const SCREEN_WIDTH: u32 = 1280;
const SCREEN_HEIGHT: u32 = 720;
const TITLE: &'static str = "Model loading";

#[derive(Debug, Default, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Vertex {
    position: Vec3,
}

impl Vertex {
    fn new(position: Vec3) -> Self {
        Self { position }
    }

    fn layout() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: &[VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            }],
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Transform {
    model_matrix: Mat4,
    normal_matrix: Mat4,
}

impl Transform {
    fn new(model_matrix: Mat4) -> Self {
        let normal_matrix = model_matrix.inverse().transpose();

        Self {
            model_matrix,
            normal_matrix,
        }
    }
}

fn main() {
    let mut event_loop = EventLoop::new();

    let logical_size = LogicalSize::new(SCREEN_WIDTH, SCREEN_HEIGHT);
    let window = WindowBuilder::new()
        .with_inner_size(logical_size)
        .with_title(TITLE)
        .with_visible(false)
        .build(&event_loop)
        .expect("failed to create a window");
    let physical_size = window.inner_size();

    let (_instance, adapter, device, queue, surface) = setup_gpu(&window);
    let mut surface_config = setup_surface(
        &surface,
        physical_size.width,
        physical_size.height,
        &adapter,
        &device,
    );

    let global_bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("bind_group_layout::global"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new(size_of::<GpuCamera>() as u64),
            },
            count: None,
        }],
    });

    let transform_bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("bind_group_layout::transform"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new(size_of::<Transform>() as u64),
            },
            count: None,
        }],
    });

    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("pipeline_layout"),
        bind_group_layouts: &[&global_bind_group_layout, &transform_bind_group_layout],
        push_constant_ranges: &[],
    });

    // Define pipelines.

    let (mut depth_texture, mut depth_texture_view) =
        create_depth_texture(&device, physical_size.width, physical_size.height);

    let shader_src = include_str!("shader.wgsl");
    let shader_module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("shader_module"),
        source: ShaderSource::Wgsl(Cow::Borrowed(shader_src)),
    });

    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("render_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &shader_module,
            entry_point: "vs_main",
            buffers: &[Vertex::layout()],
        },
        primitive: PrimitiveState {
            front_face: FrontFace::Ccw,
            cull_mode: Some(Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: CompareFunction::Less,
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &shader_module,
            entry_point: "fs_main",
            targets: &[Some(surface_config.format.into())],
        }),
        multiview: None,
    });

    // Game objects.
    let mut camera = Camera::new(&CameraDescriptor {
        aspect_ratio: SCREEN_WIDTH as f32 / SCREEN_HEIGHT as f32,
        ..Default::default()
    });

    let camera_ubo = device.create_buffer(&BufferDescriptor {
        label: Some("ubo::camera"),
        size: size_of::<GpuCamera>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let global_bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("bind_group::global"),
        layout: &global_bind_group_layout,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: camera_ubo.as_entire_binding(),
        }],
    });

    let vertices = [
        Vertex::new(Vec3::new(-0.5, 0.5, 0.0)),
        Vertex::new(Vec3::new(-0.5, -0.5, 0.0)),
        Vertex::new(Vec3::new(0.5, -0.5, 0.0)),
        Vertex::new(Vec3::new(0.5, 0.5, 0.0)),
    ];

    let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

    let transform = Transform::new(Mat4::from_scale_rotation_translation(
        Vec3::ONE,
        Quat::IDENTITY,
        Vec3::ZERO,
    ));

    let vbo = device.create_buffer(&BufferDescriptor {
        label: Some("vbo"),
        size: size_of::<Vertex>() as u64 * vertices.len() as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let ibo = device.create_buffer(&BufferDescriptor {
        label: Some("ibo"),
        size: size_of::<u32>() as u64 * indices.len() as u64,
        usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let transform_ubo = device.create_buffer(&BufferDescriptor {
        label: Some("ubo::transform"),
        size: size_of::<Transform>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let transform_bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("bind_group::transform"),
        layout: &transform_bind_group_layout,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: transform_ubo.as_entire_binding(),
        }],
    });

    queue.write_buffer(&vbo, 0, cast_slice(&vertices));
    queue.write_buffer(&ibo, 0, cast_slice(&indices));
    queue.write_buffer(&transform_ubo, 0, cast_slice(&[transform]));

    window.set_cursor_visible(false);
    window
        .set_cursor_grab(CursorGrabMode::Confined)
        .expect("failed to grab cursor");
    window.set_visible(true);

    let mut last_time = Instant::now();
    let mut running = true;
    while running {
        let current_time = Instant::now();
        let dt = (current_time - last_time).as_secs_f32();
        last_time = current_time;

        running = process_events(
            &mut event_loop,
            &window,
            &device,
            &surface,
            &mut surface_config,
            &mut depth_texture,
            &mut depth_texture_view,
            &mut camera,
            dt,
        );

        queue.write_buffer(&camera_ubo, 0, cast_slice(&[camera.get_gpu_camera()]));

        let frame = surface
            .get_current_texture()
            .expect("failed to get current swapchain texture");
        let output_texture_view = frame.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("command_encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("render_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &output_texture_view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &depth_texture_view,
                    depth_ops: Some(Operations {
                        load: LoadOp::Clear(1.0),
                        store: true,
                    }),
                    stencil_ops: Some(Operations {
                        load: LoadOp::Clear(0),
                        store: true,
                    }),
                }),
            });

            rpass.set_bind_group(0, &global_bind_group, &[]);
            rpass.set_bind_group(1, &transform_bind_group, &[]);
            rpass.set_pipeline(&pipeline);
            rpass.set_vertex_buffer(0, vbo.slice(..));
            rpass.set_index_buffer(ibo.slice(..), IndexFormat::Uint32);
            rpass.draw_indexed(0..indices.len() as u32, 0, 0..1);
        }

        queue.submit(once(encoder.finish()));
        frame.present();
    }
}

fn setup_gpu(window: &Window) -> (Instance, Adapter, Device, Queue, Surface) {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::PRIMARY,
        ..Default::default()
    });

    let surface = unsafe {
        instance
            .create_surface(window)
            .expect("failed to create a surface")
    };

    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: Some(&surface),
    }))
    .expect("failed to get a compatible adapter");

    let (device, queue) = block_on(adapter.request_device(
        &DeviceDescriptor {
            label: Some("device"),
            features: adapter.features(),
            limits: adapter.limits(),
        },
        None,
    ))
    .expect("failed to get a compatible device");

    (instance, adapter, device, queue, surface)
}

fn setup_surface(
    surface: &Surface,
    width: u32,
    height: u32,
    adapter: &Adapter,
    device: &Device,
) -> SurfaceConfiguration {
    let surface_capabilities = surface.get_capabilities(&adapter);
    let surface_format = if surface_capabilities
        .formats
        .contains(&TextureFormat::Rgba8Unorm)
    {
        TextureFormat::Rgba8Unorm
    } else if surface_capabilities
        .formats
        .contains(&TextureFormat::Bgra8Unorm)
    {
        TextureFormat::Bgra8Unorm
    } else {
        surface_capabilities.formats[0]
    };

    let config = SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width,
        height,
        present_mode: PresentMode::Fifo,
        alpha_mode: surface_capabilities.alpha_modes[0],
        view_formats: Vec::new(),
    };

    surface.configure(&device, &config);

    config
}

fn create_depth_texture(device: &Device, width: u32, height: u32) -> (Texture, TextureView) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("depth_texture"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Depth32Float,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let texture_view = texture.create_view(&TextureViewDescriptor::default());

    (texture, texture_view)
}

fn process_events(
    event_loop: &mut EventLoop<()>,
    window: &Window,
    device: &Device,
    surface: &Surface,
    surface_config: &mut SurfaceConfiguration,
    depth_texture: &mut Texture,
    depth_texture_view: &mut TextureView,
    camera: &mut Camera,
    dt: f32,
) -> bool {
    let mut quit = false;

    event_loop.run_return(|event, _, control_flow| {
        control_flow.set_wait();

        match event {
            Event::WindowEvent { window_id, event } if window.id() == window_id => match event {
                WindowEvent::CloseRequested => quit = true,

                WindowEvent::Resized(size) => {
                    surface_config.width = size.width;
                    surface_config.height = size.height;
                    surface.configure(&device, &surface_config);

                    (*depth_texture, *depth_texture_view) =
                        create_depth_texture(&device, surface_config.width, surface_config.height);
                }

                WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    surface_config.width = new_inner_size.width;
                    surface_config.height = new_inner_size.height;
                    surface.configure(&device, &surface_config);

                    (*depth_texture, *depth_texture_view) =
                        create_depth_texture(&device, surface_config.width, surface_config.height);
                }

                WindowEvent::CursorEntered { .. } => {
                    camera.set_has_mouse(true);
                }

                WindowEvent::MouseWheel { delta, .. } => {
                    if let MouseScrollDelta::LineDelta(_, y) = delta {
                        camera.zoom(y);
                    }
                }

                WindowEvent::KeyboardInput { input, .. } => {
                    if let Some(key) = input.virtual_keycode {
                        match key {
                            VirtualKeyCode::Escape if input.state == ElementState::Pressed => {
                                quit = true;
                            }
                            VirtualKeyCode::W if input.state == ElementState::Pressed => {
                                camera.move_forward(dt);
                            }
                            VirtualKeyCode::S if input.state == ElementState::Pressed => {
                                camera.move_backward(dt);
                            }
                            VirtualKeyCode::A if input.state == ElementState::Pressed => {
                                camera.skew_left(dt);
                            }
                            VirtualKeyCode::D if input.state == ElementState::Pressed => {
                                camera.skew_right(dt);
                            }

                            _ => (),
                        }
                    }
                }

                _ => (),
            },

            Event::DeviceEvent { event, .. } if camera.has_mouse() => match event {
                DeviceEvent::MouseMotion { delta } => {
                    let (x, y) = delta;

                    camera.yaw_pitch(x as f32, -y as f32);
                }

                _ => (),
            },

            Event::MainEventsCleared => control_flow.set_exit(),

            _ => (),
        }
    });

    !quit
}
