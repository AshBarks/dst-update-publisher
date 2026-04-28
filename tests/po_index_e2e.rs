mod common;

use common::load_fixture_po_index;

#[test]
fn po_index_e2e_build_and_search() {
    let index = load_fixture_po_index();

    let results = index.search_terms(&["Varg", "Nightmare Fuel", "Moleworm"]);

    let varg_result = &results[0];
    assert_eq!(varg_result.term, "Varg");
    assert!(!varg_result.candidates.is_empty());
    assert!(varg_result
        .candidates
        .iter()
        .any(|c| c.original == "Varg" && c.translation == "座狼"));

    let nf_result = &results[1];
    assert_eq!(nf_result.term, "Nightmare Fuel");
    assert!(nf_result
        .candidates
        .iter()
        .any(|c| c.original == "Nightmare Fuel" && c.translation == "噩梦燃料"));

    let mole_result = &results[2];
    assert_eq!(mole_result.term, "Moleworm");
    assert!(mole_result
        .candidates
        .iter()
        .any(|c| c.translation == "鼹鼠"));
}

#[test]
fn po_index_e2e_variant_matching() {
    let index = load_fixture_po_index();

    let results = index.search_terms(&["Vargs", "Hounds"]);

    let vargs_result = &results[0];
    assert!(
        !vargs_result.candidates.is_empty(),
        "Vargs should find via variant"
    );
    assert!(vargs_result
        .candidates
        .iter()
        .any(|c| c.original == "Varg"));

    let hounds_result = &results[1];
    assert!(
        !hounds_result.candidates.is_empty(),
        "Hounds should find results"
    );
}
