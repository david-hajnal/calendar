use reqwest::Client;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // DCR a fresh client
    let dcr = client
        .post("http://localhost:4444/oauth2/register")
        .json(&serde_json::json!({
            "client_name": "repro2",
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

    // Test 1: minimal authorize URL (one scope, short)
    let url1 = format!(
        "http://localhost:4444/oauth2/auth?client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%3A8321%2Fcallback&response_type=code&scope=commoncal.calendar.metadata.read&state=labstate01&code_challenge=li94Sv8i_jQzO5w-_CAlJHVT_EEtCjvYHIX_0gS9DtU&code_challenge_method=S256&resource=http%3A%2F%2Flocalhost%3A3001%2Fmcp"
    );
    println!("\n--- Test 1: minimal URL (len={}) ---", url1.len());
    match client.get(url1.clone()).send().await {
        Ok(resp) => println!("OK: {} loc={:?}", resp.status(), resp.headers().get("location").and_then(|v| v.to_str().ok())),
        Err(e) => println!("ERR: {e}"),
    }

    // Test 2: same but WITHOUT the resource param
    let url2 = format!(
        "http://localhost:4444/oauth2/auth?client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%3A8321%2Fcallback&response_type=code&scope=commoncal.calendar.metadata.read&state=labstate01&code_challenge=li94Sv8i_jQzO5w-_CAlJHVT_EEtCjvYHIX_0gS9DtU&code_challenge_method=S256"
    );
    println!("\n--- Test 2: no resource (len={}) ---", url2.len());
    match client.get(url2.clone()).send().await {
        Ok(resp) => println!("OK: {} loc={:?}", resp.status(), resp.headers().get("location").and_then(|v| v.to_str().ok())),
        Err(e) => println!("ERR: {e}"),
    }

    // Test 3: minimal URL but with resource (the key difference from test 2)
    // Same as test 1, just confirming.
    // Test 4: a plain GET to a known-good endpoint
    println!("\n--- Test 4: plain GET to discovery ---");
    match client.get("http://localhost:4444/.well-known/oauth-authorization-server").send().await {
        Ok(resp) => println!("OK: {}", resp.status()),
        Err(e) => println!("ERR: {e}"),
    }

    Ok(())
}
