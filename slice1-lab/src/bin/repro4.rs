use reqwest::Client;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .cookie_provider(reqwest::cookie::Jar::default().into())
        .build()?;

    // DCR
    let dcr = client
        .post("http://localhost:4444/oauth2/register")
        .json(&serde_json::json!({
            "client_name": "cookie-test2",
            "redirect_uris": ["http://127.0.0.1:8321/callback"],
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "commoncal.calendar.metadata.read"
        }))
        .send()
        .await?;
    let dcr_body: serde_json::Value = dcr.json().await?;
    let client_id = dcr_body["client_id"].as_str().unwrap().to_string();
    println!("client_id={client_id}");

    // Authorize request — check Set-Cookie
    let auth_url = format!(
        "http://localhost:4444/oauth2/auth?client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%3A8321%2Fcallback&response_type=code&scope=commoncal.calendar.metadata.read&state=labstate01&code_challenge=li94Sv8i_jQzO5w-_CAlJHVT_EEtCjvYHIX_0gS9DtU&code_challenge_method=S256&resource=http%3A%2F%2Flocalhost%3A3001%2Fmcp"
    );
    let resp = client.get(auth_url).send().await?;
    println!("authorize: {}", resp.status());
    let set_cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("<none>");
    println!("Set-Cookie: {set_cookie}");
    let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("<none>");
    println!("Location: {loc}");

    // Now make the login request and check if the cookie is sent
    // We'll use a raw TCP check: make the request and see if Hydra complains
    let login_resp = client.get(loc).send().await?;
    println!("\nlogin: {} ", login_resp.status());
    let body = login_resp.text().await?;
    println!("login body (first 300 chars): {}", &body[..body.len().min(300)]);

    Ok(())
}
