#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate gfx;
#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate smart_default;
extern crate strum;
#[macro_use]
extern crate strum_macros;

use std::{env, mem};
use std::path;
use std::time::Instant;

use log::*;
use pretty_env_logger::env_logger::Env;
use winit::event::{ElementState, Event, KeyboardInput, WindowEvent, TouchPhase};
use winit::event_loop::ControlFlow;

use crate::builtin_fs::BuiltinFS;
use crate::ggez::{Context, ContextBuilder, filesystem, GameResult};
use crate::ggez::conf::{Backend, WindowMode, WindowSetup};
use crate::ggez::event::{KeyCode, KeyMods};
use crate::ggez::graphics;
use crate::ggez::graphics::{Canvas, DrawParam, window};
use crate::ggez::input::keyboard;
use crate::ggez::mint::ColumnMatrix4;
use crate::ggez::nalgebra::Vector2;
use crate::scene::loading_scene::LoadingScene;
use crate::scene::Scene;
use crate::shared_game_state::{SharedGameState, TimingMode};
use crate::ui::UI;
use crate::ggez::event::winit_event::ModifiersState;

mod bmfont;
mod bmfont_renderer;
mod builtin_fs;
mod bullet;
mod caret;
mod common;
mod encoding;
mod engine_constants;
mod entity;
mod frame;
mod inventory;
mod ggez;
mod live_debugger;
mod macros;
mod map;
mod menu;
mod npc;
mod physics;
mod player;
mod player_hit;
mod profile;
mod rng;
mod scene;
mod shared_game_state;
mod stage;
mod sound;
mod text_script;
mod texture_set;
mod ui;
mod weapon;

struct Game {
    scene: Option<Box<dyn Scene>>,
    state: SharedGameState,
    ui: UI,
    def_matrix: ColumnMatrix4<f32>,
    start_time: Instant,
    next_tick: u64,
    loops: u64,
}

impl Game {
    fn new(ctx: &mut Context) -> GameResult<Game> {
        let s = Game {
            scene: None,
            ui: UI::new(ctx)?,
            def_matrix: DrawParam::new().to_matrix(),
            state: SharedGameState::new(ctx)?,
            start_time: Instant::now(),
            next_tick: 0,
            loops: 0,
        };

        Ok(s)
    }

    fn update(&mut self, ctx: &mut Context) -> GameResult {
        if let Some(scene) = self.scene.as_mut() {
            match self.state.timing_mode {
                TimingMode::_50Hz | TimingMode::_60Hz => {
                    while self.start_time.elapsed().as_millis() as u64 > self.next_tick && self.loops < 3 {
                        self.next_tick += self.state.timing_mode.get_delta() as u64;
                        self.loops += 1;
                    }

                    for _ in 0..self.loops {
                        scene.tick(&mut self.state, ctx)?;
                        if self.state.settings.speed_hack {
                            scene.tick(&mut self.state, ctx)?;
                        }
                    }
                }
                TimingMode::FrameSynchronized => {
                    scene.tick(&mut self.state, ctx)?;
                    if self.state.settings.speed_hack {
                        scene.tick(&mut self.state, ctx)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        graphics::clear(ctx, [0.0, 0.0, 0.0, 1.0].into());
        graphics::set_transform(ctx, DrawParam::new()
            .scale(Vector2::new(self.state.scale, self.state.scale))
            .to_matrix());
        graphics::apply_transformations(ctx)?;

        if let Some(scene) = self.scene.as_mut() {
            scene.draw(&mut self.state, ctx)?;

            graphics::set_transform(ctx, self.def_matrix);
            graphics::apply_transformations(ctx)?;
            self.ui.draw(&mut self.state, ctx, scene)?;
        }

        graphics::present(ctx)?;
        self.loops = 0;
        Ok(())
    }

    fn key_down_event(&mut self, key_code: KeyCode, _key_mod: KeyMods, repeat: bool) {
        if repeat { return; }

        // todo: proper keymaps?
        let state = &mut self.state;
        match key_code {
            KeyCode::Left => { state.key_state.set_left(true) }
            KeyCode::Right => { state.key_state.set_right(true) }
            KeyCode::Up => { state.key_state.set_up(true) }
            KeyCode::Down => { state.key_state.set_down(true) }
            KeyCode::Z => { state.key_state.set_jump(true) }
            KeyCode::X => { state.key_state.set_fire(true) }
            KeyCode::A => { state.key_state.set_weapon_prev(true) }
            KeyCode::S => { state.key_state.set_weapon_next(true) }
            KeyCode::F10 => { state.settings.debug_outlines = !state.settings.debug_outlines }
            KeyCode::F11 => { state.settings.god_mode = !state.settings.god_mode }
            KeyCode::F12 => { state.set_speed_hack(!state.settings.speed_hack) }
            _ => {}
        }
    }


    fn key_up_event(&mut self, key_code: KeyCode, _key_mod: KeyMods) {
        let state = &mut self.state;

        match key_code {
            KeyCode::Left => { state.key_state.set_left(false) }
            KeyCode::Right => { state.key_state.set_right(false) }
            KeyCode::Up => { state.key_state.set_up(false) }
            KeyCode::Down => { state.key_state.set_down(false) }
            KeyCode::Z => { state.key_state.set_jump(false) }
            KeyCode::X => { state.key_state.set_fire(false) }
            KeyCode::A => { state.key_state.set_weapon_prev(false) }
            KeyCode::S => { state.key_state.set_weapon_next(false) }
            _ => {}
        }
    }
}

#[cfg(target_os = "android")]
fn request_perms() -> GameResult {
    use jni::objects::JValue;
    use jni::objects::JObject;

    let native_activity = ndk_glue::native_activity();
    let vm_ptr = native_activity.vm();
    let vm = unsafe { jni::JavaVM::from_raw(vm_ptr) }?;
    let vm_env = vm.attach_current_thread()?;

    fn perm_name<'a, 'b, 'c>(vm_env: &'b jni::AttachGuard<'a>, name: &'c str) -> GameResult<jni::objects::JValue<'a>> {
        let class = vm_env.find_class("android/Manifest$permission")?;
        Ok(vm_env.get_static_field(class, name.to_owned(), "Ljava/lang/String;")?)
    }

    fn has_permission(vm_env: &jni::AttachGuard, activity: &jni::sys::jobject, name: &str) -> GameResult<bool> {
        let perm_granted = {
            let class = vm_env.find_class("android/content/pm/PackageManager")?;
            vm_env.get_static_field(class, "PERMISSION_GRANTED", "I")?.i()?
        };

        let perm = perm_name(vm_env, name)?;
        let activity_obj = JObject::from(*activity);
        let result = vm_env.call_method(activity_obj, "checkSelfPermission", "(Ljava/lang/String;)I", &[perm])?.i()?;
        Ok(result == perm_granted)
    }

    let str_class = vm_env.find_class("java/lang/String")?;
    let array = vm_env.new_object_array(2, str_class, JObject::null())?;
    vm_env.set_object_array_element(array, 0, perm_name(&vm_env, "READ_EXTERNAL_STORAGE")?.l()?)?;
    vm_env.set_object_array_element(array, 1, perm_name(&vm_env, "WRITE_EXTERNAL_STORAGE")?.l()?)?;
    let activity_obj = JObject::from(native_activity.activity());

    loop {
        if has_permission(&vm_env, &native_activity.activity(), "READ_EXTERNAL_STORAGE")?
            && has_permission(&vm_env, &native_activity.activity(), "WRITE_EXTERNAL_STORAGE")? {
            break;
        }

        vm_env.call_method(activity_obj, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::from(array), JValue::from(0)])?;
    }

    Ok(())
}

#[cfg(target_os = "android")]
#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn android_main() {
    println!("main invoked.");

    request_perms().expect("Failed to attach to the JVM and request storage permissions.");

    env::set_var("CAVESTORY_DATA_DIR", "/storage/emulated/0/doukutsu");
    init().unwrap();
}

fn init_ctx<P: Into<path::PathBuf>>(event_loop: &winit::event_loop::EventLoopWindowTarget<()>, resource_dir: P) -> GameResult<Context> {
    let backend = if cfg!(target_os = "android") {
        Backend::OpenGLES { major: 2, minor: 0 }
    } else {
        Backend::OpenGL { major: 3, minor: 2 }
    };

    let mut ctx = ContextBuilder::new("doukutsu-rs")
        .window_setup(WindowSetup::default().title("Cave Story (doukutsu-rs)"))
        .window_mode(WindowMode::default()
            .resizable(true)
            .min_dimensions(320.0, 240.0)
            .dimensions(854.0, 480.0))
        .add_resource_path(resource_dir)
        .add_resource_path(path::PathBuf::from(str!("./")))
        .backend(backend)
        .build(event_loop)?;

    ctx.filesystem.mount_vfs(Box::new(BuiltinFS::new()));

    Ok(ctx)
}

pub fn init() -> GameResult {
    pretty_env_logger::env_logger::from_env(Env::default().default_filter_or("info"))
        .filter(Some("gfx_device_gl::factory"), LevelFilter::Warn)
        .init();

    let resource_dir = if let Ok(data_dir) = env::var("CAVESTORY_DATA_DIR") {
        path::PathBuf::from(data_dir)
    } else if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let mut path = path::PathBuf::from(manifest_dir);
        path.push("data");
        path
    } else {
        path::PathBuf::from("data")
    };

    info!("Resource directory: {:?}", resource_dir);
    info!("Initializing engine...");

    let event_loop = winit::event_loop::EventLoop::new();
    let mut context: Option<Context> = None;
    let mut game: Option<Game> = None;

    #[cfg(target_os = "android")]
        {
            loop {
                match ndk_glue::native_window().as_ref() {
                    Some(_) => {
                        println!("NativeScreen Found:{:?}", ndk_glue::native_window());
                        break;
                    }
                    None => ()
                }
            }
        }

    context = Some(init_ctx(&event_loop, resource_dir.clone())?);

    event_loop.run(move |event, target, flow| {
        if let Some(ctx) = &mut context {
            ctx.timer_context.tick();
            ctx.process_event(&event);

            if let Some(game) = &mut game {
                game.ui.handle_events(ctx, &event);
            } else {
                let mut new_game = Game::new(ctx).unwrap();
                new_game.state.next_scene = Some(Box::new(LoadingScene::new()));
                game = Some(new_game);
            }
        }

        match event {
            Event::Resumed => {
                #[cfg(target_os = "android")]
                if context.is_none() {
                    context = Some(init_ctx(target, resource_dir.clone()).unwrap());
                }

                if let Some(game) = &mut game {
                    game.loops = 0;
                }
            }
            Event::Suspended => {
                #[cfg(target_os = "android")]
                    {
                        context = None;
                    }
                if let Some(game) = &mut game {
                    game.loops = 0;
                }
            }
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::CloseRequested => {
                        if let Some(game) = &mut game {
                            game.state.shutdown();
                        }
                        *flow = ControlFlow::Exit;
                    }
                    WindowEvent::Resized(_) => {
                        if let (Some(ctx), Some(game)) = (&mut context, &mut game) {
                            game.state.handle_resize(ctx).unwrap();
                            game.state.lightmap_canvas = Canvas::with_window_size(ctx).unwrap();
                            //old_school_gfx_glutin_ext::WindowUpdateExt::update_gfx(graphics::window(ctx), &mut game.ui.main_color, &mut game.ui.main_depth);
                        }
                    }
                    WindowEvent::Touch(touch) => {
                        if let Some(game) = &mut game {

                        }
                    }
                    WindowEvent::KeyboardInput {
                        input:
                        KeyboardInput {
                            state: el_state,
                            virtual_keycode: Some(keycode),
                            modifiers,
                            ..
                        },
                        ..
                    } => {
                        if let (Some(ctx), Some(game)) = (&mut context, &mut game) {
                            match el_state {
                                ElementState::Pressed => {
                                    let repeat = keyboard::is_key_repeated(ctx);
                                    game.key_down_event( keycode, modifiers.into(), repeat);
                                }
                                ElementState::Released => {
                                    game.key_up_event(keycode, modifiers.into());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::RedrawRequested(win) => {
                if let (Some(ctx), Some(game)) = (&mut context, &mut game) {
                    if win == window(ctx).window().id() {
                        game.draw(ctx).unwrap();
                    }
                }
            }
            Event::MainEventsCleared => {
                if let (Some(ctx), Some(game)) = (&mut context, &mut game) {
                    game.update(ctx).unwrap();

                    #[cfg(target_os = "android")]
                        game.draw(ctx).unwrap(); // redraw request is unimplemented on shitdroid

                    window(ctx).window().request_redraw();

                    if game.state.shutdown {
                        log::info!("Shutting down...");
                        *flow = ControlFlow::Exit;
                        return;
                    }

                    if game.state.next_scene.is_some() {
                        mem::swap(&mut game.scene, &mut game.state.next_scene);
                        game.state.next_scene = None;

                        game.scene.as_mut().unwrap().init(&mut game.state, ctx).unwrap();
                    }
                }
            }
            _ => {}
        }
    });
}
