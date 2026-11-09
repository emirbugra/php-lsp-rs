//! End-to-end tests of the Symfony and CodeIgniter 4 support on `tests/fixtures/{symfony,ci4}`.

mod common;

use common::{fixture, locations, pos, Client};
use serde_json::{json, Value};

fn warnings(diags: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = diags
        .iter()
        .filter(|d| d["code"].as_str().is_some_and(|c| c.starts_with("laravel-unknown-")))
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

fn labels(r: &Value) -> Vec<String> {
    let items = r.get("items").unwrap_or(r);
    items.as_array().unwrap().iter().filter_map(|i| i["label"].as_str().map(str::to_string)).collect()
}

#[test]
fn symfony_routes_and_templates() {
    let mut c = Client::start(&fixture("symfony"));
    let (uri, text) = c.open("src/Controller/BlogController.php", None);
    let td = json!({ "uri": uri });

    let r = c.request(
        "textDocument/completion",
        json!({ "textDocument": td, "position": pos(&text, "generateUrl('')", 0, 13) }),
    );
    let got = labels(&r);
    for want in ["home", "blog_index", "blog_app_blog_show"] {
        assert!(got.iter().any(|l| l == want), "{want} missing in {got:?}");
    }

    let def = c.request(
        "textDocument/definition",
        json!({ "textDocument": td, "position": pos(&text, "'blog_index'", 0, 3) }),
    );
    assert_eq!(locations(&def), vec![("BlogController.php".to_string(), 9)]);
    let def =
        c.request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "'home'", 0, 2) }));
    assert_eq!(locations(&def), vec![("routes.yaml".to_string(), 1)]);
    let def = c.request(
        "textDocument/definition",
        json!({ "textDocument": td, "position": pos(&text, "blog/index.html.twig", 0, 3) }),
    );
    assert_eq!(locations(&def), vec![("index.html.twig".to_string(), 1)]);

    assert_eq!(
        warnings(&c.diagnostics(&uri)),
        ["Route 'blog_indx' not found — did you mean 'blog_index'?", "View 'blog/missing.html.twig' not found"]
    );
}

#[test]
fn codeigniter_strings_views_and_factories() {
    let mut c = Client::start(&fixture("ci4"));
    let (uri, text) = c.open("app/Controllers/Home.php", None);
    let td = json!({ "uri": uri });

    assert_eq!(
        warnings(&c.diagnostics(&uri)),
        [
            "Route 'hom' not found — did you mean 'home'?",
            "Translation 'App.welcom' not found — did you mean 'App.welcome'?",
            "View 'users/missing' not found",
        ]
    );
    let def = c
        .request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "users/index", 0, 2) }));
    assert_eq!(locations(&def), vec![("index.php".to_string(), 1)]);
    let def = c
        .request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "App.welcome", 0, 5) }));
    assert_eq!(locations(&def), vec![("App.php".to_string(), 3)]);

    // `model('UserModel')` is typed, so its methods resolve
    let def =
        c.request("textDocument/definition", json!({ "textDocument": td, "position": pos(&text, "findAll", 0, 2) }));
    assert_eq!(locations(&def), vec![("UserModel.php".to_string(), 9)]);

    // controller handlers in the routes file
    let (ruri, rtext) = c.open("app/Config/Routes.php", None);
    let rtd = json!({ "uri": ruri });
    let r = c.request(
        "textDocument/completion",
        json!({ "textDocument": rtd, "position": pos(&rtext, "Home::users", 0, 0) }),
    );
    let got = labels(&r);
    assert!(got.iter().any(|l| l == "Home::index") && got.iter().any(|l| l == "Home::users"), "{got:?}");
    let def = c.request(
        "textDocument/definition",
        json!({ "textDocument": rtd, "position": pos(&rtext, "Home::index", 0, 7) }),
    );
    assert_eq!(locations(&def), vec![("Home.php".to_string(), 6)]);

    // the view gets the controller's data with types
    let (vuri, vtext) = c.open("app/Views/users/index.php", None);
    let vtd = json!({ "uri": vuri });
    let r =
        c.request("textDocument/completion", json!({ "textDocument": vtd, "position": pos(&vtext, "getName", 0, 0) }));
    assert!(labels(&r).iter().any(|l| l == "getName"), "{:?}", labels(&r));
    let def =
        c.request("textDocument/definition", json!({ "textDocument": vtd, "position": pos(&vtext, "baseURL", 0, 2) }));
    assert_eq!(locations(&def), vec![("App.php".to_string(), 6)]);
}
