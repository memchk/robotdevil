mod perms;

use std::{collections::HashSet, env};

use serenity::prelude::*;
use serenity::{
    async_trait,
    client::bridge::gateway::GatewayIntents,
    framework::standard::{
        macros::{command, group},
        CommandResult, StandardFramework,
    },
    Client,
};
use serenity::{
    framework::standard::help_commands, framework::standard::macros::help,
    framework::standard::Args, framework::standard::CommandGroup, framework::standard::HelpOptions,
    model::prelude::*,
};

use log::error;
use perms::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {

    async fn ready(&self, ctx: Context, _data_about_bot: Ready) {
        perms::load_banned_users(ctx).await;
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        if let Err(e) = member_role(&ctx, &reaction, true).await {
            error!("{}", e);
        }
    }

    async fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        if let Err(e) = member_role(&ctx, &reaction, false).await {
            error!("{}", e);
        }
    }
}

// Commands (for now)
#[command]
async fn ping(ctx: &Context, msg: &Message) -> CommandResult {
    println!("Attempting Pang");
    msg.channel_id.say(&ctx.http, "Pong!").await?;

    Ok(())
}

// Command configuration.
#[group]
#[required_permissions("ADMINISTRATOR")]
#[commands(ping, post_rules_msg, timeout, release)]
struct Admin;

struct BotKVStore;
impl TypeMapKey for BotKVStore {
    type Value = kv::Store;
}

async fn get_store(ctx: &Context) -> kv::Store {
    ctx.data.read().await.get::<BotKVStore>().unwrap().clone()
}

#[help]
async fn my_help(
    context: &Context,
    msg: &Message,
    args: Args,
    help_options: &'static HelpOptions,
    groups: &[&'static CommandGroup],
    owners: HashSet<UserId>,
) -> CommandResult {
    let _ = help_commands::with_embeds(context, msg, args, help_options, groups, owners).await;
    Ok(())
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().expect("Failed to load .env");
    env_logger::init();

    let token = env::var("DISCORD_TOKEN").expect("Expected a bot token in DISCORD_TOKEN.");

    let framework = StandardFramework::new()
        .configure(|c| c.prefix("~"))
        .group(&ADMIN_GROUP)
        .help(&MY_HELP);

    let store =
        kv::Store::new(kv::Config::new("robotdevil_data")).expect("Can not open bot data store");

    let mut client = Client::new(&token)
        .event_handler(Handler)
        .add_intent(GatewayIntents::DIRECT_MESSAGES)
        .add_intent(GatewayIntents::GUILD_MESSAGES)
        .add_intent(GatewayIntents::GUILD_MESSAGE_REACTIONS)
        .add_intent(GatewayIntents::GUILDS)
        .add_intent(GatewayIntents::GUILD_PRESENCES)
        .add_intent(GatewayIntents::GUILD_MEMBERS)
        .framework(framework)
        .await
        .expect("Err creating client");

    {
        let mut data = client.data.write().await;
        data.insert::<BotKVStore>(store);
    }

    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
}
