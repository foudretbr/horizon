use horizon_world::{
    registry::BlockRegistry,
    worldgen::{
        generator::{GeneratorData, OverworldGenerator},
        native_generate::{self, NativeGenerateHooks, NativeLodColumn},
    },
};
use std::{path::Path, sync::Mutex};

type HookFn = Box<dyn Fn(&str, [i32; 2], i64, bool, &mut NativeLodColumn<'_>) + Send + Sync>;
static CURRENT_HOOK: Mutex<Option<HookFn>> = Mutex::new(None);
/// The hook is process-wide: tests that replace it run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn ensure_hooks_installed() {
    let _ = native_generate::install(NativeGenerateHooks {
        chunk_generate: |_, _, _, _, _, _| {},
        lod_column: |dimension, column, seed, flat, column_out| {
            if let Ok(guard) = CURRENT_HOOK.lock()
                && let Some(hook) = &*guard
            {
                hook(dimension, column, seed, flat, column_out);
            }
        },
    });
}

fn load_test_generator() -> Option<(std::sync::Arc<GeneratorData>, OverworldGenerator)> {
    let root = std::env::var_os("HORIZON_TEST_RESOURCES")
        .or_else(|| std::env::var_os("HORIZON_RESOURCES"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local/horizon-client/26.3")
        });
    if !root.join("worldgen/world_presets.json").exists() {
        return None;
    }
    let registry: &'static BlockRegistry = Box::leak(Box::new(
        horizon_world::level::data::load_registry(&root).ok()?,
    ));
    let data = GeneratorData::load(&root.join("worldgen"), registry).ok()?;
    let generator = OverworldGenerator::without_structures(data.clone(), 1234).ok()?;
    Some((data, generator))
}

#[test]
fn native_lod_column_replaces_grid_before_vanilla_evaluation() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    ensure_hooks_installed();
    assert!(native_generate::has_lod_column_hook());

    let Some((data, generator)) = load_test_generator() else {
        eprintln!("skipping: resources not available");
        return;
    };
    let mut worker = generator.worker();

    // 1. When the hook replaces all columns in a grid:
    let stone = data
        .registry()
        .default_state("minecraft:stone")
        .expect("stone");
    let plains = data.biome_id("minecraft:plains").expect("plains");

    {
        let mut guard = CURRENT_HOOK.lock().unwrap();
        *guard = Some(Box::new(move |_dimension, [x, z], _seed, _flat, out| {
            // Give each column a distinct height based on coordinates
            let height = 70 + (x.abs() % 10) + (z.abs() % 10);
            out.replace(vec![(50, height, stone)], Some(plains));
        }));
    }

    let grid = worker.lod_grid(0, 0, 16, 2, false).expect("lod_grid");
    assert_eq!(grid.len(), 4);
    for (i, (sample, spans)) in grid.iter().enumerate() {
        let x = (i % 2) as i32 * 16;
        let z = (i / 2) as i32 * 16;
        let expected_height = 70 + (x % 10) + (z % 10);

        let sample = sample.as_ref().expect("surface sample");
        assert_eq!(sample.height, expected_height);
        assert_eq!(sample.state, stone);
        assert_eq!(sample.biome, plains);
        assert_eq!(sample.water, None);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0], (50, expected_height, stone));
    }

    // 2. When the hook replaces only a specific column:
    let special_x = 16;
    let special_z = 0;
    let desert = data.biome_id("minecraft:desert").expect("desert");

    {
        let mut guard = CURRENT_HOOK.lock().unwrap();
        *guard = Some(Box::new(move |_dimension, [x, z], _seed, _flat, out| {
            if x == special_x && z == special_z {
                out.replace(vec![(40, 99, stone)], Some(desert));
            }
        }));
    }

    let mixed = worker.lod_grid(0, 0, 16, 2, false).expect("mixed grid");
    assert_eq!(mixed.len(), 4);

    // Column [16, 0] must be the mod's answer
    let (mod_sample, mod_spans) = &mixed[1]; // index 1: x = 16, z = 0
    let mod_sample = mod_sample.as_ref().expect("mod sample");
    assert_eq!(mod_sample.height, 99);
    assert_eq!(mod_sample.state, stone);
    assert_eq!(mod_sample.biome, desert);
    assert_eq!(mod_spans.len(), 1);
    assert_eq!(mod_spans[0], (40, 99, stone));

    // The other columns must be vanilla's (not height 99)
    for (i, (sample, _)) in mixed.iter().enumerate() {
        if i != 1 {
            let sample = sample.as_ref().expect("vanilla sample");
            assert_ne!(sample.height, 99);
        }
    }

    // 3. Clear hook: vanilla behavior preserved
    {
        let mut guard = CURRENT_HOOK.lock().unwrap();
        *guard = None;
    }
}

#[test]
fn surface_sample_and_column_consult_hook_first() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    ensure_hooks_installed();

    let Some((data, generator)) = load_test_generator() else {
        eprintln!("skipping: resources not available");
        return;
    };
    let mut worker = generator.worker();
    let stone = data
        .registry()
        .default_state("minecraft:stone")
        .expect("stone");
    let plains = data.biome_id("minecraft:plains").expect("plains");

    {
        let mut guard = CURRENT_HOOK.lock().unwrap();
        *guard = Some(Box::new(move |_dimension, [x, z], _seed, _flat, out| {
            if x == 500 && z == 500 {
                out.replace(vec![(30, 85, stone)], Some(plains));
            }
        }));
    }

    let sample = worker.surface_sample(500, 500).expect("sample");
    assert_eq!(sample.height, 85);
    assert_eq!(sample.state, stone);
    assert_eq!(sample.biome, plains);

    let (col_sample, col_spans) = worker.surface_column(500, 500);
    assert_eq!(col_sample.unwrap().height, 85);
    assert_eq!(col_spans, vec![(30, 85, stone)]);

    {
        let mut guard = CURRENT_HOOK.lock().unwrap();
        *guard = None;
    }
}
