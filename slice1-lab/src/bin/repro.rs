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
            "client_name": "repro",
            "redirect_uris": ["http://127.0.0.1:8321/callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": "commoncal.calendar.metadata.read"
        }))
        .send()
        .await?;
    let dcr_status = dcr.status();
    let dcr_body: serde_json::Value = dcr.json().await?;
    let client_id = dcr_body["client_id"].as_str().unwrap().to_string();
    println!("DCR: {dcr_status} client_id={client_id}");

    // Now the authorize request (same as lab-prove)
    let mut auth_url = url::Url::parse("http://localhost:4444/oauth2/auth")?;
    auth_url.query_pairs_mut()
        .clear()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", "http://127.0.0.1:8321/callback")
        .append_pair("response_type", "code")
        .append_pair("scope", "commoncal.calendar.metadata.read commoncal.availability.read commoncal.event.read.basic commoncal.event.read.details commoncal.event.create commoncal.event.update commoncal.event.delete commoncal.reminder.read commoncal.reminder.write evil.unknown.scope offline_access")
        .append_pair("state", "lab-state-0001")
        .append_pair("code_challenge", "li94Sv8i_jQzO5w-_CAlJHVT_EEtCjvYHIX_0gS9DtU")
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", "http://localhost:3001/mcp");

    let auth_str = auth_url.to_string();
    println!("AUTH URL len={}", auth_str.len());
    println!("AUTH URL: {auth_str}");

    let result = client.get(auth_url.clone()).send().await;
    match result {
        Ok(resp) => {
            let status = resp.status();
            let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("<none>");
            println!("AUTH: {status} location={loc}");
        }
        Err(e) => {
            println!("AUTH ERROR: {e}");
        }
    }
    Ok(())
}
