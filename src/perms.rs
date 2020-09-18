use std::error::Error as StdError;
use std::str::FromStr;

use serenity::framework::standard::{macros::command, CommandResult};
use serenity::prelude::*;
use serenity::{framework::standard::Args, model::prelude::*};

use chrono::{prelude::*, Duration};
use kv::{Bucket, Msgpack, Store};
use log::{error, info};

const MEMBER_ROLE: RoleId = RoleId(753785520122757211);

const UNBAN_MESSAGE : &'_ str = "Your temp-ban has expired. Please re-read the rules and click the reaction to recieve your perms, or message one of the admins if there is an issue.";
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredMessage {
    channel: ChannelId,
    msg: MessageId,
}

fn stored_messages(store: &Store) -> Bucket<&str, kv::Msgpack<StoredMessage>> {
    store
        .bucket(Some("messages"))
        .expect("Could not open kv::messages bucket.")
}

fn timeout_users(store: &Store) -> Bucket<&str, kv::Msgpack<DateTime<Utc>>> {
    store
        .bucket(Some("timeout"))
        .expect("Could not open kv::timeout bucket.")
}

#[command("makerules")]
pub async fn post_rules_msg(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    msg.reply(
        ctx,
        "Respond with the new post. 'cancel' will stop the process.",
    )
    .await?;
    let store = super::get_store(ctx).await;

    if let Some(reaction) = msg.author.await_reply(ctx).channel_id(msg.channel_id).await {
        if reaction.content == "cancel" {
            msg.channel_id.say(ctx, "Action Canceled.").await?;
        } else {
            let rules_channel = ChannelId::from_str(args.rest())?;
            let stored_messages = stored_messages(&store);
            let new_message = rules_channel.say(ctx, &reaction.content).await?;
            // :white_check_mark:
            new_message.react(ctx, '✅').await?;

            stored_messages
                .set(
                    "rules",
                    Msgpack(StoredMessage {
                        channel: rules_channel,
                        msg: new_message.id,
                    }),
                )
                .unwrap();
            stored_messages.flush_async().await?;

            msg.channel_id.say(ctx, "Rules posted.").await?;
        }
    }

    Ok(())
}

async fn release_user(ctx: Context, user: UserId) {
    let store = super::get_store(&ctx).await;
    let timeout_users = timeout_users(&store);

    // Message the user if able to let them know they are unbanned.
    if let Ok(dm) = user.create_dm_channel(&ctx).await {
        // This message is on a best effort basis, we don't care if it fails.
        dm.say(&ctx, UNBAN_MESSAGE).await.ok();
    }

    // Remove the persistant record.
    let _ = timeout_users.remove(&user.to_string()[..]);
    timeout_users.flush_async().await.unwrap();
}

#[command]
pub async fn timeout(ctx: &Context, msg: &Message, mut args: Args) -> CommandResult {
    if let Ok(user) = args.single::<UserId>() {
        if let Ok(time) = args.single::<humantime::Duration>() {
            let x : std::time::Duration = time.into();
            let release_at : DateTime<Utc> = Utc::now() + Duration::from_std(x).unwrap();

            let store = super::get_store(&ctx).await;
            let timeout_users = timeout_users(&store);
            let stored_messages = stored_messages(&store);

            timeout_users.set(&user.to_string()[..], Msgpack(release_at)).unwrap();
            timeout_users.flush_async().await?;

            // Drop the users role.
            let mut member = msg
                        .guild_id
                        .expect("Message should not be in DMs!")
                        .member(ctx, user)
                        .await?;

            unban_task(ctx.clone(), user, release_at).ok();

            member.remove_role(ctx, MEMBER_ROLE).await?;
            member.disconnect_from_voice(ctx).await?;

            // If it exists, drop their reaction.
            if let Some(Msgpack(rules_message)) = stored_messages.get("rules")? {
                rules_message.channel.delete_reaction(ctx, rules_message.msg, Some(user), ReactionType::from('✅')).await.ok();
            }

            if let Ok(dm) = user.create_dm_channel(ctx).await {
                dm.say(ctx, format!("You have been temporarily removed from the discord. Your ban will expire at **{}**, at which time you will recieve more information. Please contact @memchk for questions.", release_at)).await.ok();
            }

            msg.reply(ctx, format!("Timeout applied. Will expire: {}", release_at)).await.ok();
        }
    } else {
        msg.reply(ctx, "Could not parse user.").await?;
    }

    Ok(())
}

#[command]
pub async fn release(ctx: &Context, msg: &Message, mut args: Args) -> CommandResult {
    if let Ok(user) = args.single::<UserId>() {
        release_user(ctx.clone(), user).await;
        msg.reply(ctx, "User has been released from timeout.").await?;
    } else {
        msg.reply(ctx, "Could not parse user.").await?;
    }

    Ok(())
}

pub async fn member_role(ctx: &Context, reaction: &Reaction, added: bool) -> CommandResult {
    let store = super::get_store(ctx).await;
    let stored_messages = stored_messages(&store);
    let timeout_users = timeout_users(&store);

    if let Some(Msgpack(rules_message)) = stored_messages.get("rules")? {
        dbg!(reaction, added);
        if reaction.channel_id == rules_message.channel && reaction.message_id == rules_message.msg
        {
            let user = reaction.user_id.unwrap();
            if !timeout_users.contains(&user.to_string()[..])? {
                if reaction.emoji == ReactionType::from('✅') {
                    let mut member = reaction
                        .guild_id
                        .expect("Message should not be in DMs!")
                        .member(ctx, user)
                        .await?;

                    if added {
                        if let Err(e) = member.add_role(ctx, MEMBER_ROLE).await {
                            error!("Could not add role to uid {}, {}", user, e);
                        } else {
                            info!("Added member role to uid: {}", user);
                        }
                    } else {
                        member.remove_role(ctx, MEMBER_ROLE).await?;
                        info!("Removed member role from uid: {}", user);
                    }
                }
            }
            else if added {
                reaction.delete(ctx).await.unwrap();
            }
        }
    }

    Ok(())
}

use tokio::sync::oneshot;

pub fn unban_task(
    ctx: Context,
    user: UserId,
    release_at: DateTime<Utc>,
) -> Result<oneshot::Sender<()>, Box<dyn StdError>> {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let wait_time = release_at.signed_duration_since(Utc::now()).to_std()?;
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::delay_for(wait_time) => {
                release_user(ctx, user).await;
            },
            Ok(_) = cancel_rx => { /* Do nothing, this cancels the timer. */ }
        }
    });

    Ok(cancel_tx)
}

pub async fn load_banned_users(ctx: Context) {
    let store = super::get_store(&ctx).await;
    let timeout_users = timeout_users(&store);
    for banned_user in timeout_users.iter() {
        let banned_user = banned_user.unwrap();

        // Shitty error handling, I know.
        let uid = UserId::from_str(banned_user.key().unwrap()).unwrap();
        let Msgpack(release_at) = banned_user.value().unwrap();
        unban_task(ctx.clone(), uid, release_at).unwrap();
    }
}