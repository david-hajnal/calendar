use reqwest::Client;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Client with redirect following DISABLED
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    // DCR a fresh client (POST, no redirect)
    let dcr = client
        .post("http://localhost:4444/oauth2/register")
        .json(&serde_json::json!({
            "client_name": "repro3",
            "redirect_uris": ["http://127.0.0.1:8321/callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "commoncal.calendar.metadata.read"
        }))
        .send()
        .await?;
    let dcr_body: serde_json::Value = dcr.json().await?;
    let client_id = dcr_body["client_id"].as_str().unwrap().to_string();
    println!("client_id={client_id}");

    // Authorize request with redirect following DISABLED
    let url = format!(
        "http://localhost:4444/oauth2/auth?client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%3A8321%2Fcallback&response_type=code&scope=commoncal.calendar.metadata.read&state=labstate01&code_challenge=li94Sv8i_jQzO5w-_CAlJHVT_EEtCjvYHIX_0gS9DtU&code_challenge_method=S256&resource=http%3A%2F%2Flocalhost%3A3001%2Fmcp"
    );
    println!("\n--- authorize (redirects disabled) ---");
    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("<none>");
            println!("OK: {status}");
            println!("Location: {loc}");
        }
        Err(e) => println!("ERR: {e}"),
    }
    Ok(())
}
