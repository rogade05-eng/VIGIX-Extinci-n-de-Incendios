//! El perfil de demostración incluye casos de prueba con resultados esperados calculados de forma
//! independiente (Python). El motor debe reproducirlos todos.

use std::fs;
use std::path::PathBuf;
use vigix_core::json::Json;
use vigix_core::profile::{Level, Profile};
use vigix_core::selftest;

fn demo() -> Profile {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json");
    Profile::from_json(Json::parse(&fs::read_to_string(p).unwrap()).unwrap()).unwrap()
}

#[test]
fn el_motor_reproduce_todos_los_casos_independientes_del_perfil_demo() {
    let p = demo();
    let res = selftest::run(&p);
    assert!(res.len() >= 10, "se esperaban >= 10 casos, hay {}", res.len());
    for r in &res {
        assert!(r.passed, "caso {} ({}) falló: {:?}", r.id, r.system, r.messages);
    }
    // cubre todos los sistemas declarados en el ámbito
    for s in p.raw.at("scope.systems").unwrap().items() {
        let sys = s.as_str().unwrap();
        assert!(res.iter().any(|r| r.system == sys), "falta un caso para {}", sys);
    }
    // sigue siendo no verificado por ser de demostración
    assert_eq!(p.level, Level::Unverified);
}

#[test]
fn alterar_un_valor_esperado_hace_fallar_el_autotest() {
    let mut j = {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/demo/DEMO-NO-NORMATIVO_0.1.0.json");
        Json::parse(&fs::read_to_string(p).unwrap()).unwrap()
    };
    j.set_at("test_cases.1.expected.0.value", Json::num(999.0));
    let p = Profile::from_json(j).unwrap();
    let res = selftest::run(&p);
    assert!(res.iter().any(|r| !r.passed));
    assert_eq!(selftest::summary_json(&res).field("failed").as_f64(), Some(1.0));
}
