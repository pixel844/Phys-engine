use bevy::prelude::*;

const SQUARE_SIZE: f32 = 50.0;
const OUT_OF_BOUNDS_TIME: f32 = 5.0;

fn main() {
    App::new()
    .add_plugins(DefaultPlugins)
    .insert_resource(PhysicsConfig::default())

    .add_message::<Contact>()
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            toggle_friction,
            adjust_restitution,
            spawn_square_on_space,
            remove_square_on_hover,
            drag_square,
            draw_velocity_vectors,
            display_momentum_info,
        ),
    )
    // FixedUpdate uses a fixed timestep; Res<Time> in this schedule provides fixed delta [web:71]
    .add_systems(
        FixedUpdate,
        (
            clear_forces,
            apply_gravity,
            integrate_velocity,
            detect_circle_contacts, // multi-directional normals (2D)
        solve_contacts,         // impulses + positional correction
        integrate_position,
        check_out_of_bounds,    // only print here
        )
        .chain(),
    )
    .run();
}

#[derive(Resource)]
struct PhysicsConfig {
    friction_enabled: bool,
    restitution: f32,    // 0..=1 (↑↓)
    gravity: Vec2,       // can set to Vec2::new(0.0, -980.0) if desired
    linear_damping: f32, // per-second damping when friction_enabled
    slop: f32,           // penetration slop
    percent: f32,        // positional correction factor
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            friction_enabled: true,
            restitution: 0.8,
            gravity: Vec2::ZERO,
            linear_damping: 2.0,
            slop: 0.01,
            percent: 0.8,
        }
    }
}

#[derive(Component)]
struct MainCamera;

#[derive(Component)]
struct Square;

#[derive(Component, Default)]
struct Velocity(Vec2);

#[derive(Component, Default)]
struct Force(Vec2);

#[derive(Component, Copy, Clone)]
struct Mass {
    mass: f32,
    inv: f32,
}
impl Mass {
    fn new(mass: f32) -> Self {
        Self {
            mass,
            inv: if mass > 0.0 { 1.0 / mass } else { 0.0 },
        }
    }
}

// Circle collider gives fully 2D collision normals (multi-directional response)
#[derive(Component, Copy, Clone)]
struct ColliderCircle {
    radius: f32,
}

#[derive(Component)]
struct Dragging {
    offset: Vec2,
    last_cursor_world: Vec2,
}

#[derive(Component)]
struct OutOfBoundsTimer(f32);

#[derive(Component)]
struct MomentumText;

#[derive(Message, Copy, Clone)]
struct Contact {
    a: Entity,
    b: Entity,
    normal: Vec2,     // points from A -> B
    penetration: f32, // overlap depth
}

fn setup(mut commands: Commands) {
    commands.spawn((Camera2d, MainCamera));

    commands.spawn((
        Text::new(""),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(10.0),
                    left: Val::Px(10.0),
                    ..default()
                    },
                    TextFont {
                        font_size: 18.0,
                        ..default()
                    },
                    TextColor(Color::WHITE),
                    MomentumText,
    ));
}

// ---------- INPUT ----------
fn toggle_friction(keys: Res<ButtonInput<KeyCode>>, mut cfg: ResMut<PhysicsConfig>) {
    if keys.just_pressed(KeyCode::KeyF) {
        cfg.friction_enabled = !cfg.friction_enabled;
    }
}

fn adjust_restitution(keys: Res<ButtonInput<KeyCode>>, mut cfg: ResMut<PhysicsConfig>) {
    if keys.just_pressed(KeyCode::ArrowUp) {
        cfg.restitution = (cfg.restitution + 0.1).min(1.0);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        cfg.restitution = (cfg.restitution - 0.1).max(0.0);
    }
}

fn spawn_square_on_space(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
                         mut meshes: ResMut<Assets<Mesh>>,
                         mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, cam_tf)) = camera_q.single() else { return };

    let Some(cursor) = window.cursor_position() else { return };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, cursor) else { return };

    commands.spawn((
        Mesh2d(meshes.add(Rectangle::new(SQUARE_SIZE, SQUARE_SIZE))),
                    MeshMaterial2d(materials.add(Color::srgb(0.2, 0.7, 0.9))),
                    Transform::from_translation(world.extend(0.0)),
                    Square,
                    Velocity::default(),
                    Force::default(),
                    Mass::new(1.0),
                    ColliderCircle {
                        radius: SQUARE_SIZE * 0.5,
                    },
    ));
}

fn remove_square_on_hover(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
                          squares: Query<(Entity, &Transform, &ColliderCircle), With<Square>>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Ok((camera, cam_tf)) = camera_q.single() else { return };

    let Some(cursor) = window.cursor_position() else { return };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, cursor) else { return };

    for (e, t, c) in squares.iter() {
        let p = t.translation.truncate();
        if world.distance(p) <= c.radius {
            commands.entity(e).despawn();
            break;
        }
    }
}

fn drag_square(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
               mut squares: Query<
               (
                   Entity,
                &mut Transform,
                &ColliderCircle,
                &mut Velocity,
                Option<&Dragging>,
               ),
               With<Square>,
               >,
) {
    let Ok(window) = windows.single() else { return };
    let Ok((camera, cam_tf)) = camera_q.single() else { return };

    let Some(cursor) = window.cursor_position() else { return };
    let Ok(cursor_world) = camera.viewport_to_world_2d(cam_tf, cursor) else { return };

    // Start drag
    if mouse.just_pressed(MouseButton::Left) {
        for (e, t, c, mut v, dragging) in squares.iter_mut() {
            if dragging.is_some() {
                continue;
            }
            let p = t.translation.truncate();
            if cursor_world.distance(p) <= c.radius {
                v.0 = Vec2::ZERO;
                commands.entity(e).insert(Dragging {
                    offset: p - cursor_world,
                    last_cursor_world: cursor_world,
                });
                break;
            }
        }
        return;
    }

    // Continue drag
    if mouse.pressed(MouseButton::Left) {
        let dt = time.delta_secs().max(1e-6);

        for (e, mut t, _c, mut v, dragging) in squares.iter_mut() {
            if let Some(d) = dragging {
                let target = cursor_world + d.offset;
                t.translation = target.extend(0.0);

                // Maintain a kinematic velocity so collision impulses affect other bodies
                v.0 = (cursor_world - d.last_cursor_world) / dt;

                commands.entity(e).insert(Dragging {
                    offset: d.offset,
                    last_cursor_world: cursor_world,
                });
            }
        }
        return;
    }

    // End drag
    if mouse.just_released(MouseButton::Left) {
        for (e, _t, _c, _v, dragging) in squares.iter() {
            if dragging.is_some() {
                commands.entity(e).remove::<Dragging>();
            }
        }
    }
}

// ---------- FIXEDUPDATE PHYSICS ----------
fn clear_forces(mut q: Query<&mut Force>) {
    for mut f in &mut q {
        f.0 = Vec2::ZERO;
    }
}

fn apply_gravity(cfg: Res<PhysicsConfig>, mut q: Query<(&Mass, &mut Force), Without<Dragging>>) {
    for (m, mut f) in &mut q {
        f.0 += cfg.gravity * m.mass;
    }
}

fn integrate_velocity(
    cfg: Res<PhysicsConfig>,
    time: Res<Time>, // fixed dt in FixedUpdate [web:71]
    mut q: Query<(&Mass, &Force, &mut Velocity), Without<Dragging>>,
) {
    let dt = time.delta_secs();

    for (m, f, mut v) in &mut q {
        v.0 += f.0 * m.inv * dt;

        if cfg.friction_enabled && cfg.linear_damping > 0.0 {
            let damp = (1.0 - cfg.linear_damping * dt).clamp(0.0, 1.0);
            v.0 *= damp;
        }
    }
}

// Multi-directional contact generation: normal is center-to-center unit vector
fn detect_circle_contacts(
    q: Query<(Entity, &Transform, &ColliderCircle), With<Square>>,
                          mut writer: MessageWriter<Contact>,
) {
    let mut combos = q.iter_combinations();

    while let Some([(ea, ta, ca), (eb, tb, cb)]) = combos.fetch_next() {
        let pa = ta.translation.truncate();
        let pb = tb.translation.truncate();
        let delta = pb - pa;

        let r = ca.radius + cb.radius;
        let dist2 = delta.length_squared();

        if dist2 < r * r {
            let dist = dist2.sqrt().max(1e-6);
            let normal = delta / dist; // ANY direction in 2D
            let penetration = r - dist;

            writer.write(Contact {
                a: ea,
                b: eb,
                normal,
                penetration,
            });
        }
    }
}

// Impulse solve + positional correction, with dragged squares treated as kinematic (inv_mass=0)
fn solve_contacts(
    cfg: Res<PhysicsConfig>,
    mut reader: MessageReader<Contact>,
    mut q: Query<(&mut Transform, &mut Velocity, &Mass, Option<&Dragging>), With<Square>>,
) {
    for c in reader.read() {
        let Ok([(mut ta, mut va, ma, da), (mut tb, mut vb, mb, db)]) =
        q.get_many_mut([c.a, c.b])
        else {
            continue;
        };

        let inv_a = if da.is_some() { 0.0 } else { ma.inv };
        let inv_b = if db.is_some() { 0.0 } else { mb.inv };
        let inv_sum = inv_a + inv_b;
        if inv_sum <= 0.0 {
            continue;
        }

        // Positional correction
        let pen = (c.penetration - cfg.slop).max(0.0);
        if pen > 0.0 {
            let correction = c.normal * (pen * cfg.percent / inv_sum);
            if da.is_none() {
                ta.translation -= (correction * inv_a).extend(0.0);
            }
            if db.is_none() {
                tb.translation += (correction * inv_b).extend(0.0);
            }
        }

        // Impulse along normal
        let rv = vb.0 - va.0;
        let vel_along_normal = rv.dot(c.normal);

        // If separating, skip
        if vel_along_normal > 0.0 {
            continue;
        }

        let e = cfg.restitution.clamp(0.0, 1.0);
        let j = -(1.0 + e) * vel_along_normal / inv_sum;
        let impulse = c.normal * j;

        if da.is_none() {
            va.0 -= impulse * inv_a;
        }
        if db.is_none() {
            vb.0 += impulse * inv_b;
        }
    }
}

fn integrate_position(
    time: Res<Time>, // fixed dt in FixedUpdate [web:71]
    mut q: Query<(&mut Transform, &Velocity), Without<Dragging>>,
) {
    let dt = time.delta_secs();
    for (mut t, v) in &mut q {
        t.translation += (v.0 * dt).extend(0.0);
    }
}

fn check_out_of_bounds(
    mut commands: Commands,
    time: Res<Time>, // fixed dt in FixedUpdate [web:71]
    windows: Query<&Window>,
    mut q: Query<(Entity, &Transform, Option<&mut OutOfBoundsTimer>), With<Square>>,
) {
    let Ok(window) = windows.single() else { return };

    let margin = 200.0;
    let max_x = window.width() / 2.0 + margin;
    let max_y = window.height() / 2.0 + margin;

    for (e, t, timer) in &mut q {
        let p = t.translation.truncate();
        let out = p.x.abs() > max_x || p.y.abs() > max_y;

        if out {
            if let Some(mut timer) = timer {
                timer.0 += time.delta_secs();
                if timer.0 >= OUT_OF_BOUNDS_TIME {
                    commands.entity(e).despawn();
                    println!("Square deleted (out of bounds for 5s)");
                }
            } else {
                commands.entity(e).insert(OutOfBoundsTimer(0.0));
            }
        } else if timer.is_some() {
            commands.entity(e).remove::<OutOfBoundsTimer>();
        }
    }
}

// ---------- DEBUG/UI ----------
fn draw_velocity_vectors(q: Query<(&Transform, &Velocity), With<Square>>, mut gizmos: Gizmos) {
    for (t, v) in &q {
        let p = t.translation.truncate();
        if v.0.length() > 0.1 {
            gizmos.arrow_2d(p, p + v.0 * 0.1, Color::srgb(1.0, 1.0, 0.0));
        }
    }
}

fn display_momentum_info(
    cfg: Res<PhysicsConfig>,
    bodies: Query<(&Velocity, &Mass), With<Square>>,
                         mut text_q: Query<&mut Text, With<MomentumText>>,
) {
    let mut p_total = Vec2::ZERO;
    let mut ke_total = 0.0;
    let mut count = 0usize;

    for (v, m) in &bodies {
        p_total += v.0 * m.mass;
        ke_total += 0.5 * m.mass * v.0.length_squared();
        count += 1;
    }

    let Ok(mut text) = text_q.single_mut() else { return };

    let kind = if cfg.restitution >= 0.95 {
        "ELASTIC"
    } else if cfg.restitution <= 0.05 {
        "INELASTIC"
    } else {
        "PARTIAL"
    };

    **text = format!(
        "Total Momentum in the scene: ({:.1}, {:.1}) kg x m/s \ntotal Kinetic Energy: {:.1} J\nSquares: {}\nFriction: {}\nRestitution of e: {:.1} ({})\n\nControls:\n press SPACE to spawn | R to remove cube | hold on a cube to Drag/throw\n F to toggle friction | arrow up and down keys = elasticity",
                     p_total.x,
                     p_total.y,
                     ke_total,
                     count,
                     if cfg.friction_enabled { "ON" } else { "OFF" },
                         cfg.restitution,
                     kind
    );
}
