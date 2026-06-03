use anyhow::Result;
use crate::api::LmsClient;
use crate::config;

pub async fn print_info() -> Result<()> {
    let cfg = config::Config::load()?;
    let config_path = config::config_path();

    println!("lyrtui v{} — Info", env!("CARGO_PKG_VERSION"));

    // ── Configuration ──────────────────────────────────────────────────────────
    println!("\nCONFIGURATION  ({})", config_path.display());
    println!("  server:          {}:{}", cfg.host, cfg.port);
    println!("  url:             {}", cfg.base_url());
    match (&cfg.username, &cfg.password) {
        (Some(u), Some(_)) => println!("  auth:            username={}, password=set", u),
        (Some(u), None) => println!("  auth:            username={}, password=not set", u),
        _ => println!("  auth:            none"),
    }
    match &cfg.default_player {
        Some(id) => println!("  default player:  {}", id),
        None => println!("  default player:  none"),
    }
    println!(
        "  auto-discover:   {} (mask: {})",
        yn(cfg.auto_discover),
        cfg.broadcast_mask
    );
    println!("  nerd icons:      {}", yn(cfg.use_nerd_icons));
    println!("  image protocol:  {}", cfg.image_protocol);
    println!("  full art mode:   {}", yn(cfg.full_art_mode));
    println!("  auto colors:     {}", yn(!cfg.disable_auto_colors));
    println!("  global volume:   {}", yn(cfg.global_volume_control));

    let client = LmsClient::new(cfg.base_url(), cfg.credentials());

    // ── Server ─────────────────────────────────────────────────────────────────
    println!("\nSERVER  (live)");
    match client.get_server_info().await {
        Err(e) => println!("  [unreachable: {}]", e),
        Ok(info) => {
            if let Some(v) = &info.version {
                println!("  version:         {}", v);
            }
            if let Some(n) = &info.name {
                println!("  name:            {}", n);
            }
            if let Some(ip) = &info.ip {
                println!("  ip:              {}", ip);
            }
            if let Some(m) = &info.mac {
                println!("  mac:             {}", m);
            }
            if let Some(id) = &info.uuid {
                println!("  uuid:            {}", id);
            }
            if let Some(c) = info.player_count {
                println!("  players:         {}", c);
            }
        }
    }

    // ── Players ────────────────────────────────────────────────────────────────
    let players = match client.get_players_detailed().await {
        Err(e) => {
            println!("\nPLAYERS\n  [unreachable: {}]", e);
            return Ok(());
        }
        Ok(p) => p,
    };

    println!("\nPLAYERS  ({})", players.len());

    for (i, p) in players.iter().enumerate() {
        let status = if p.power == 0 {
            "OFF".to_string()
        } else if p.is_playing == 1 {
            "PLAYING".to_string()
        } else {
            "STOPPED".to_string()
        };
        let indicator = if p.power == 0 {
            "○"
        } else if p.is_playing == 1 {
            "▶"
        } else {
            "■"
        };
        println!("\n  [{}] {:<38} {} {}", i + 1, p.name, indicator, status);
        println!("      id:          {}", p.playerid);
        if let Some(ip) = &p.ip {
            println!("      ip:          {}", ip);
        }
        match (&p.model, &p.modelname) {
            (Some(m), Some(mn)) => println!("      model:       {} ({})", m, mn),
            (Some(m), None) => println!("      model:       {}", m),
            _ => {}
        }
        if let Some(fw) = &p.firmware {
            println!("      firmware:    {}", fw);
        }
        if let Some(uid) = &p.uuid
            && !uid.is_empty()
        {
            println!("      uuid:        {}", uid);
        }
        println!(
            "      power:       {}    connected: {}",
            yn(p.power == 1),
            yn(p.connected == 1)
        );

        if p.power == 1 {
            match client.get_now_playing(&p.playerid).await {
                Ok(np) => {
                    println!("      volume:      {}", np.volume);
                    if !np.title.is_empty() {
                        println!("      now:         \"{}\" — {}", np.title, np.artist);
                        let mut meta = np.album.clone();
                        if let Some(y) = np.year {
                            meta = format!("{} ({})", meta, y);
                        }
                        if let (Some(idx), Some(total)) =
                            (np.playlist_cur_index, np.playlist_tracks)
                        {
                            meta = format!("{}  [track {} / {}]", meta, idx + 1, total);
                        }
                        if !meta.is_empty() {
                            println!("                   {}", meta);
                        }
                    } else {
                        println!("      now:         (nothing playing)");
                    }
                }
                Err(e) => println!("      status:      [error: {}]", e),
            }
        }
    }

    Ok(())
}

pub fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

async fn resolve_player(client: &LmsClient, cfg: &config::Config) -> Result<String> {
    if let Some(id) = &cfg.default_player {
        return Ok(id.clone());
    }
    let players = client.get_players().await?;
    players
        .into_iter()
        .next()
        .map(|p| p.playerid)
        .ok_or_else(|| anyhow::anyhow!("no players found on server"))
}

fn format_track(title: &str, artist: &str) -> String {
    match (title.is_empty(), artist.is_empty()) {
        (true, _) => "(unknown)".to_string(),
        (false, true) => format!("\"{}\"", title),
        (false, false) => format!("\"{}\" — {}", title, artist),
    }
}

/// Load config and resolve the default player for one-shot CLI commands.
async fn cli_player() -> Result<(LmsClient, String)> {
    let cfg = config::Config::load()?;
    let client = LmsClient::new(cfg.base_url(), cfg.credentials());
    let pid = resolve_player(&client, &cfg).await?;
    Ok((client, pid))
}

pub async fn cmd_play_pause() -> Result<()> {
    let (client, pid) = cli_player().await?;
    let np = client.get_now_playing(&pid).await?;
    let track = format_track(&np.title, &np.artist);
    if np.is_playing {
        client.pause(&pid).await?;
        println!("paused  {}", track);
    } else {
        client.play(&pid).await?;
        println!("playing {}", track);
    }
    Ok(())
}

pub async fn cmd_next() -> Result<()> {
    let (client, pid) = cli_player().await?;
    client.next(&pid).await?;
    if let Ok(np) = client.get_now_playing(&pid).await {
        println!("next  {}", format_track(&np.title, &np.artist));
    }
    Ok(())
}

pub async fn cmd_prev() -> Result<()> {
    let (client, pid) = cli_player().await?;
    client.prev(&pid).await?;
    if let Ok(np) = client.get_now_playing(&pid).await {
        println!("prev  {}", format_track(&np.title, &np.artist));
    }
    Ok(())
}
