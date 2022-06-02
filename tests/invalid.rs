mod common;

macro_rules! invalid_rule {
    ($rule:expr) => {
        paste::item! {
            #[test]
            fn [< invalid_ $rule >] () {
                let rule = common::load_rule($rule);
                assert_eq!(rule.is_err(), true);
            }
        }
    };
}

invalid_rule!("identifier_missing");