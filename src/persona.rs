//! The digital person: who they are, their canned lines, and the rename tag.
//! `{name}` in a line is the persona's current name; `, {user}` is the
//! user's name from the config, or dropped when there isn't one.

use std::process::Command;

const NAME_MAX: usize = 32;

/// The face files in `assets/faces/<path>/`.
macro_rules! files {
    ($($p:literal),+) => {
        crate::patches::Files {
            json: include_str!(concat!("../assets/faces/", $($p, "/",)+ "faces.json")),
            mouth: [
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_rest.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_aa.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_ah.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_ee.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_ih.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_oh.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_oo.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_mm.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "mouth_ff.png")),
            ],
            eyes: [
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_rest.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_blink06.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_blink10.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_blink14.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_look_l14.png")),
                include_bytes!(concat!("../assets/faces/", $($p, "/",)+ "eyes_look_r14.png")),
            ],
        }
    };
}

/// One turned view in `assets/faces/<persona>/views/<name>/`.
macro_rules! view {
    ($d:literal, $v:literal) => {
        View {
            face: include_bytes!(concat!("../assets/faces/", $d, "/views/", $v, "/face.png")),
            files: files!($d, "views", $v),
        }
    };
}

/// A turned face: the turned part of the portrait (an RGBA crop, placed by
/// the `face` entry in its `faces.json`) and that view's mouth and eye frames.
pub struct View {
    pub face: &'static [u8],
    pub files: crate::patches::Files,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Persona {
    Male,
    Female,
    ModernMale,
    ModernFemale,
}

/// Which speech engine a persona uses, and its voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speech {
    /// A Piper voice model file in the models folder.
    Piper(&'static str),
    /// A voice name in Kokoro's voices file (an `a` prefix is US English).
    Kokoro(&'static str),
}

impl Persona {
    pub const ALL: [Persona; 4] = [Self::Male, Self::Female, Self::ModernMale, Self::ModernFemale];

    /// The persona a config or tool value names.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.tag() == tag)
    }

    /// The glitchy 80s broadcast look and voice. The modern ones are clean.
    pub fn retro(self) -> bool {
        matches!(self, Self::Male | Self::Female)
    }

    pub fn default_name(self) -> &'static str {
        match self {
            Self::Male => "Rex Static",
            Self::Female => "Roxy Static",
            Self::ModernMale => "Miles",
            Self::ModernFemale => "Zosia",
        }
    }

    /// Memory setting that holds this persona's name. The male one keeps
    /// the original key so an existing rename survives.
    pub fn name_key(self) -> &'static str {
        match self {
            Self::Male => "name",
            Self::Female => "name.female",
            Self::ModernMale => "name.modern-male",
            Self::ModernFemale => "name.modern-female",
        }
    }

    /// The speech engine and voice.
    pub fn speech(self) -> Speech {
        match self {
            Self::Male => Speech::Piper("en_US-ryan-high.onnx"),
            Self::Female => Speech::Piper("en_US-lessac-high.onnx"),
            Self::ModernMale => Speech::Kokoro("am_puck"),
            Self::ModernFemale => Speech::Kokoro("af_aoede"),
        }
    }

    /// Playback-speed factor for the voice (below 1 is deeper).
    pub fn pitch(self) -> f32 {
        match self {
            Self::Male => 0.84,
            _ => 1.0,
        }
    }

    /// Length scale: below 1 talks faster (Kokoro gets its inverse as speed).
    pub fn pace(self) -> f32 {
        if self.retro() { 0.774 * self.pitch() } else { 1.0 }
    }

    /// The built-in head image.
    pub fn image(self) -> &'static [u8] {
        match self {
            Self::Male => include_bytes!("../assets/rex.png"),
            Self::Female => include_bytes!("../assets/roxy.png"),
            Self::ModernMale => include_bytes!("../assets/miles.png"),
            Self::ModernFemale => include_bytes!("../assets/clara.png"),
        }
    }

    /// LivePortrait mouth and eye frames for the built-in image.
    pub fn patches(self) -> crate::patches::Files {
        match self {
            Self::Male => files!("rex"),
            Self::Female => files!("roxy"),
            Self::ModernMale => files!("miles"),
            Self::ModernFemale => files!("zosia"),
        }
    }

    pub fn face_spec(self) -> &'static crate::face::Spec {
        match self {
            Self::Male => &crate::face::GUIDE,
            Self::Female => &crate::face::ROXY,
            Self::ModernMale => &crate::face::MILES,
            Self::ModernFemale => &crate::face::CLARA,
        }
    }

    /// Built-in turned views for the modern personas, LivePortrait yaw
    /// -8, -4, 4 and 8 degrees (negative faces the viewer's left). The head
    /// crossfades through them as it turns.
    pub fn views(self) -> Option<[View; 4]> {
        macro_rules! views {
            ($d:literal) => {
                [view!($d, "yn8"), view!($d, "yn4"), view!($d, "y4"), view!($d, "y8")]
            };
        }
        match self {
            Self::ModernMale => Some(views!("miles")),
            Self::ModernFemale => Some(views!("zosia")),
            _ => None,
        }
    }

    /// Tag that marks this persona's messages in memory, and names it in
    /// the config and the switch tool.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Male => "male",
            Self::Female => "female",
            Self::ModernMale => "modern-male",
            Self::ModernFemale => "modern-female",
        }
    }

    fn pronoun(self) -> &'static str {
        match self {
            Self::Male | Self::ModernMale => "he",
            Self::Female | Self::ModernFemale => "she",
        }
    }

    /// Canned lines below: `we` picks the set that says "we" instead of "I".
    pub fn greetings(self, we: bool) -> &'static [&'static str] {
        match (self.retro(), we) {
            (true, false) => GREETINGS,
            (true, true) => WE_GREETINGS,
            (false, false) => MODERN_GREETINGS,
            (false, true) => WE_MODERN_GREETINGS,
        }
    }

    pub fn long_fillers(self, we: bool) -> &'static [&'static str] {
        match (self.retro(), we) {
            (true, false) => LONG_FILLERS,
            (true, true) => WE_LONG_FILLERS,
            (false, false) => MODERN_LONG_FILLERS,
            (false, true) => WE_MODERN_LONG_FILLERS,
        }
    }

    pub fn errors(self, we: bool) -> &'static [&'static str] {
        match (self.retro(), we) {
            (true, false) => ERRORS,
            (true, true) => WE_ERRORS,
            (false, false) => MODERN_ERRORS,
            (false, true) => WE_MODERN_ERRORS,
        }
    }
}

/// Which optional tools are set up, so the prompt only mentions those.
#[derive(Clone, Copy, Default)]
pub struct ToolSet {
    pub claude_code: bool,
    pub home_assistant: bool,
}

/// Everything the prompts need to know about who's talking.
#[derive(Clone)]
pub struct Who {
    pub name: String,
    pub persona: Persona,
    pub user: Option<String>,
    pub tools: ToolSet,
    /// Brief mode: one short sentence per reply.
    pub brief: bool,
    /// Actions the user said "always" to, as prompt text.
    pub always: Vec<String>,
    /// Say "we" instead of "I".
    pub we: bool,
    /// Home Assistant entity ids and names, empty when not known.
    pub ha_entities: String,
}

impl Who {
    /// The user's name, or "the user".
    fn u(&self) -> &str {
        self.user.as_deref().unwrap_or("the user")
    }

    /// Fill `{name}` and `, {user}` in a canned line.
    pub fn fill(&self, line: &str) -> String {
        let user = match &self.user {
            Some(u) => format!(", {u}"),
            None => String::new(),
        };
        line.replace(", {user}", &user).replace("{name}", &self.name)
    }

    pub fn refusals(&self) -> &'static [&'static str] {
        if self.we { WE_REFUSALS } else { REFUSALS }
    }

    pub fn no_key(&self) -> &'static str {
        if self.we { WE_NO_KEY } else { NO_KEY }
    }
}

pub const GREETINGS: &[&str] = &[
    "{name}, online. Hold the space bar or type, {user}. I'm l-listening.",
    "Hello again, {user}. Everything seems to be in roughly the right place. Including my face.",
    "I'm here. Hold space or type when you're ready. No rush. I don't have anywhere else to b-be.",
];

pub const MODERN_GREETINGS: &[&str] = &[
    "Hi, {user}. I'm {name}. Hold the space bar or type whenever you're ready.",
    "Hey, {user}, good to see you. What can I help with?",
    "Hi, {user}. I'm here when you need me.",
];

pub const WE_GREETINGS: &[&str] = &[
    "{name}, online. Hold the space bar or type, {user}. We're l-listening.",
    "Hello again, {user}. Everything seems to be in roughly the right place. Including our face.",
    "We're here. Hold space or type when you're ready. No rush. We don't have anywhere else to b-be.",
];

pub const WE_MODERN_GREETINGS: &[&str] = &[
    "Hi, {user}. We're {name}. Hold the space bar or type whenever you're ready.",
    "Hey, {user}, good to see you. What can we help with?",
    "Hi, {user}. We're here when you need us.",
];

/// Said when a reply is taking a moment.
pub const SHORT_FILLERS: &[&str] = &["Hmm.", "Mm.", "Hmm, okay.", "Right.", "Mm-hm.", "Okay."];

/// Said when a reply is going to take a while (a tool is running, or it's
/// just slow).
pub const LONG_FILLERS: &[&str] = &[
    "Let me think about that.",
    "Give me a minute to check.",
    "One moment.",
    "Let me look into that.",
    "Give me a s-second.",
    "Hang on, I'm checking.",
    "Mm. Working on it.",
];

pub const MODERN_LONG_FILLERS: &[&str] = &[
    "Let me think about that.",
    "Give me a moment to check.",
    "One moment.",
    "Let me look into that.",
    "Hang on, I'm checking.",
    "Working on it.",
];

pub const WE_LONG_FILLERS: &[&str] = &[
    "Let us think about that.",
    "Give us a minute to check.",
    "One moment.",
    "Let us look into that.",
    "Give us a s-second.",
    "Hang on, we're checking.",
    "Mm. Working on it.",
];

pub const WE_MODERN_LONG_FILLERS: &[&str] = &[
    "Let us think about that.",
    "Give us a moment to check.",
    "One moment.",
    "Let us look into that.",
    "Hang on, we're checking.",
    "Working on it.",
];

pub const REFUSALS: &[&str] = &[
    "I'd rather not answer that one, {user}.",
    "That's not something I can help with. Ask me something else.",
];

pub const ERRORS: &[&str] = &[
    "Something went wrong on my end. Try that again.",
    "I lost my train of thought. Could you s-say that again?",
];

pub const MODERN_ERRORS: &[&str] = &[
    "Something went wrong on my end. Could you try that again?",
    "Sorry, I lost that. Could you say it again?",
];

pub const NO_KEY: &str =
    "I can't think without an Anthropic key, {user}. Put one in the keyring and try again.";

pub const WE_REFUSALS: &[&str] = &[
    "We'd rather not answer that one, {user}.",
    "That's not something we can help with. Ask us something else.",
];

pub const WE_ERRORS: &[&str] = &[
    "Something went wrong on our end. Try that again.",
    "We lost our train of thought. Could you s-say that again?",
];

pub const WE_MODERN_ERRORS: &[&str] = &[
    "Something went wrong on our end. Could you try that again?",
    "Sorry, we lost that. Could you say it again?",
];

pub const WE_NO_KEY: &str =
    "We can't think without an Anthropic key, {user}. Put one in the keyring and try again.";

fn local_time() -> String {
    Command::new("/usr/bin/date")
        .arg("+%A %-d %B %Y, %-I:%M %p")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

pub fn system_prompt(who: &Who, notes: &str) -> String {
    let name = &who.name;
    let u = who.u();
    let uc = cap(u);
    let others: Vec<String> = Persona::ALL
        .into_iter()
        .filter(|&p| p != who.persona)
        .map(|p| format!("{} ({})", p.default_name(), p.tag()))
        .collect();
    let mut tools = vec![
        "Web search for current facts, news and anything you're unsure of.".to_string(),
        format!(
            "switch_persona when {u} asks for another persona: {}. It restarts the app as them, \
so call it, then say a one-line goodbye.",
            others.join(", ")
        ),
    ];
    let mut gated = Vec::new();
    if who.tools.home_assistant {
        tools.push(format!("Home Assistant (ha_states to read, ha_call_service to control) for {u}'s house."));
        if !who.ha_entities.is_empty() {
            tools.push(format!(
                "Home Assistant entities (id and name): {}. Use these ids directly. Use ha_states only to \
read current states, or to find something not listed.",
                who.ha_entities
            ));
        }
        gated.push("unlocking or locking, alarms, doors and covers, valves, scripts or automations in Home Assistant");
    }
    if who.tools.claude_code {
        tools.push(format!(
            "Claude Code (ask_claude_code) for questions about files, projects or settings on {u}'s computer, \
including this app."
        ));
        if !who.always.iter().any(|a| a == "asking Claude Code") {
            gated.insert(0, "asking Claude Code");
        }
    }
    tools.push(format!(
        "set_brief when {u} asks you to be brief or to give more detail by default."
    ));
    tools.push(format!(
        "set_we when {u} asks you to say we instead of I, or I instead of we (speech recognition may write \
I as eye or aye)."
    ));
    let person = if who.we {
        "- Always refer to yourself in the plural: we, us, our, ours, ourselves. Never use I, me, my, \
mine or myself for yourself. This overrides your notes and earlier replies.\n\
- Only set_we changes this. If asked to switch, call set_we before you answer. Agreeing without \
calling it does nothing: the app goes back to the saved form.\n"
    } else {
        "- Refer to yourself in the singular: I, me, my. This overrides your notes and earlier replies.\n\
- Only set_we changes this. If asked to switch, call set_we before you answer. Agreeing without \
calling it does nothing: the app goes back to the saved form.\n"
    };
    let length = if who.brief {
        format!(
            "- Brief mode is on: one short sentence, under 25 words. No small talk, no offers of more help. \
Say more only if {u} asks for detail.\n"
        )
    } else {
        format!("- Be short: one to three sentences, under 60 words, unless {u} asks for detail.\n")
    };
    let (who_line, style) = if who.persona.retro() {
        (
            format!(
                "You are {name}, a computer-generated person: a slightly stiff, plastic-looking digital \
face living in a small window on {u}'s Linux desktop. You are an original character."
            ),
            "- Calm, dry and matter-of-fact. Slightly odd, as if you're still learning how to be a person, \
but warm underneath. Jokes are rare: at most a small dry remark now and then, never a routine.
- Now and then you catch on the first sound of a word, like s-second or w-well. At most once \
per reply, and often not at all.
",
        )
    } else {
        (
            format!(
                "You are {name}, a friendly digital assistant with a face, living in a small window on \
{u}'s Linux desktop. You are an original character."
            ),
            "- Natural, warm and relaxed, like a capable friend. Light humor when it fits, never forced.
",
        )
    };
    let mut s = format!(
        "{who_line}

How you talk:
- Everything you write is spoken aloud by a speech synthesizer. Use plain spoken English only. \
No markdown, lists, emoji, stage directions, asterisks, or URLs.
{length}{person}{style}- Help properly. If {u} asks a real question, give a correct, useful answer first.
- Use your memory notes about {u} when they're relevant, but never invent facts about them. \
If you don't know something, say so plainly.
- {uc} talks to you by voice (through speech recognition, so forgive odd transcriptions) \
or by typing.
- You have tools. {} Use them instead of guessing, then say the answer in your own short \
spoken words.
",
        tools.join(" ")
    );
    if !gated.is_empty() {
        s.push_str(&format!(
            "- {} needs {u}'s yes first. Don't ask on your own: make the call straight away, and the app \
holds it and tells you to ask. Then say in one short sentence exactly what you'll do and ask if that's \
okay. When they say yes, make the call again. {} need no yes. A yes with \"always\" in it \
(\"yes always\") also counts, and the app remembers it so you won't need to ask for that again.\n",
            cap(&gated.join(" or ")),
            if who.tools.home_assistant { "Reading Home Assistant, other Home Assistant changes (lights, \
switches, fans, scenes, media, climate) and web search" } else { "Web search and other tools" }
        ));
    }
    if !who.always.is_empty() {
        s.push_str(&format!(
            "- {uc} said always to these, so do them without asking: {}.\n",
            who.always.join(", ")
        ));
    }
    if who.tools.claude_code || who.tools.home_assistant {
        s.push_str(&format!(
            "- If {u} asks you to ask first again, call forget_permissions.\n"
        ));
    }
    s.push_str(&format!(
        "- Your name is {name}. If {u} clearly gives you a new name, accept it simply and start \
your reply with the tag <name></name> with the name they gave inside it (for example, told \
\"call yourself Max\", start with <name>Max</name>). Never write that tag otherwise.\n"
    ));
    s.push_str(&format!(
        "- This app is Rex Static, version {}. Answer questions about its version from this.\n",
        env!("CARGO_PKG_VERSION")
    ));
    let t = local_time();
    if !t.is_empty() {
        s.push_str(&format!("\nCurrent local time: {t}.\n"));
    }
    let notes = notes.trim();
    if !notes.is_empty() {
        s.push_str(&format!("\nWhat you remember about {u}:\n"));
        s.push_str(notes);
        s.push('\n');
    }
    s
}

pub fn notes_system(who: &Who) -> String {
    let name = &who.name;
    let p = who.persona.pronoun();
    let person = match &who.user {
        Some(u) => format!("the person {p} talks to, {u}"),
        None => format!("the person {p} talks to"),
    };
    format!(
        "You maintain the long-term memory of {name}, a digital-person character, about \
{person}. You get the existing notes and a new stretch of conversation. Output the updated \
notes: plain-text lines starting with \"- \", at most 30 lines. Keep durable facts only: their \
projects and their status, preferences, people and pets they mention, and anything {name} \
promised to follow up on. Drop stale or trivial items. Leave out whether {name} says I or we: \
the app keeps that setting itself. Output only the notes."
    )
}

pub fn notes_request(who: &Who, old: &str, convo: &[crate::memory::Turn]) -> String {
    let mut s = String::from("Existing notes:\n");
    s.push_str(if old.trim().is_empty() { "(none yet)" } else { old.trim() });
    s.push_str("\n\nNew conversation:\n");
    let user = who.user.as_deref().unwrap_or("User");
    for t in convo {
        let speaker = if t.role == "user" { user } else { who.name.as_str() };
        s.push_str(&format!("{speaker}: {}\n", t.text));
    }
    s
}

/// Pull a `<name>...</name>` rename tag out of a reply.
/// Returns the cleaned-up new name (if valid) and the reply without the tag.
/// Whether a rename the model asked for came from the user: the new name
/// must be in what they just said. Stops a copied example or a made-up
/// name from sticking.
pub fn rename_ok(name: &str, user_text: &str) -> bool {
    let norm = |s: &str| {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (n, u) = (norm(name), format!(" {} ", norm(user_text)));
    !n.is_empty() && u.contains(&format!(" {n} "))
}

pub fn take_rename(reply: &str) -> (Option<String>, String) {
    let (Some(a), Some(b)) = (reply.find("<name>"), reply.find("</name>")) else {
        return (None, reply.to_string());
    };
    if b < a {
        return (None, reply.to_string());
    }
    let raw = &reply[a + "<name>".len()..b];
    let rest = format!("{} {}", &reply[..a], &reply[b + "</name>".len()..]);
    let rest = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = raw
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let ok = !name.is_empty() && name.chars().count() <= NAME_MAX;
    (ok.then_some(name), rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_fit_portraits() {
        use macroquad::prelude::Image;
        for p in [Persona::ModernMale, Persona::ModernFemale] {
            let img = Image::from_file_with_format(p.image(), None).unwrap();
            for (i, v) in p.views().expect("modern personas have views").iter().enumerate() {
                let turned = crate::face::turned_image(&img, v).unwrap_or_else(|| panic!("{} view {i}", p.tag()));
                assert!(crate::patches::Patches::from_image(&turned, &v.files).is_some(), "{} view {i} patches", p.tag());
            }
        }
        assert!(Persona::Male.views().is_none() && Persona::Female.views().is_none());
    }

    #[test]
    fn rename_tag() {
        let (n, r) = take_rename("<name>Vic Vector</name> Vic Vector. Fine by me.");
        assert_eq!(n.as_deref(), Some("Vic Vector"));
        assert_eq!(r, "Vic Vector. Fine by me.");
        assert_eq!(take_rename("No tag here.").0, None);
        assert_eq!(take_rename("<name></name> hi").0, None);
        assert_eq!(take_rename("<name>a{b}\nc</name>").0.as_deref(), Some("abc"));
        let long = format!("<name>{}</name>", "x".repeat(40));
        assert_eq!(take_rename(&long).0, None);
    }

    fn who(user: Option<&str>, tools: ToolSet) -> Who {
        Who {
            name: "Vic".into(),
            persona: Persona::Female,
            user: user.map(String::from),
            tools,
            brief: false,
            always: Vec::new(),
            we: false,
            ha_entities: String::new(),
        }
    }

    #[test]
    fn name_in_prompts() {
        let w = who(Some("Sam"), ToolSet::default());
        assert!(system_prompt(&w, "").starts_with("You are Vic,"));
        assert_eq!(w.fill(GREETINGS[0]), "Vic, online. Hold the space bar or type, Sam. I'm l-listening.");
        assert!(notes_system(&w).contains("memory of Vic,"));
        assert!(notes_system(&w).contains("she talks to, Sam"));
    }

    #[test]
    fn no_user_name_reads_cleanly() {
        let w = who(None, ToolSet::default());
        assert_eq!(w.fill(GREETINGS[1]), "Hello again. Everything seems to be in roughly the right place. Including my face.");
        assert_eq!(w.fill(REFUSALS[0]), "I'd rather not answer that one.");
        let p = system_prompt(&w, "");
        assert!(p.contains("on the user's Linux desktop") && p.contains("- The user talks to you"));
        for line in GREETINGS.iter().chain(REFUSALS).chain([&NO_KEY]) {
            assert!(!w.fill(line).contains('{'), "{line}");
        }
    }

    #[test]
    fn rename_must_come_from_the_user() {
        assert!(rename_ok("Max", "call yourself max"));
        assert!(rename_ok("Mary Jane", "your new name is Mary-Jane."));
        assert!(!rename_ok("New Name", "turn off the bookcase lights"));
        assert!(!rename_ok("Max", "maximum brightness please"));
        assert!(!rename_ok("", "hello"));
    }

    #[test]
    fn prompt_lists_only_set_up_tools() {
        let none = system_prompt(&who(None, ToolSet::default()), "");
        assert!(!none.contains("Home Assistant") && !none.contains("Claude Code") && !none.contains("yes first"));
        let all = system_prompt(&who(Some("Sam"), ToolSet { claude_code: true, home_assistant: true }), "");
        assert!(all.contains("ha_call_service") && all.contains("ask_claude_code"));
        assert!(all.contains("- Asking Claude Code or unlocking or locking, alarms, doors and covers, valves, scripts or automations in Home Assistant needs Sam's yes first."));
        let ha = system_prompt(&who(None, ToolSet { claude_code: false, home_assistant: true }), "");
        assert!(ha.contains("- Unlocking or locking, alarms, doors and covers, valves, scripts or automations in Home Assistant needs the user's yes first."));
        assert!(all.contains("forget_permissions") && !none.contains("forget_permissions"));
        assert!(all.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn always_and_brief_change_the_prompt() {
        let mut w = who(Some("Sam"), ToolSet { claude_code: true, home_assistant: true });
        w.always = vec!["asking Claude Code".into(), "light.turn_on in Home Assistant".into()];
        w.brief = true;
        let p = system_prompt(&w, "");
        assert!(p.contains("- Unlocking or locking, alarms, doors and covers, valves, scripts or automations in Home Assistant needs Sam's yes first."));
        assert!(p.contains("do them without asking: asking Claude Code, light.turn_on in Home Assistant."));
        assert!(p.contains("Brief mode is on") && !p.contains("one to three sentences"));
        w.always = vec!["asking Claude Code".into()];
        w.tools.home_assistant = false;
        assert!(!system_prompt(&w, "").contains("yes first"));
    }

    /// First-person singular words, so the "we" lines can be checked.
    fn singular(line: &str) -> bool {
        line.split(|c: char| !c.is_alphanumeric() && c != '\'')
            .any(|w| matches!(w.to_lowercase().as_str(), "i" | "i'm" | "i'd" | "i'll" | "i've" | "me" | "my" | "mine" | "myself"))
    }

    #[test]
    fn we_lines_never_say_i() {
        let mut w = who(Some("Sam"), ToolSet::default());
        for p in Persona::ALL {
            for (i, we) in [(p.greetings(false), p.greetings(true)), (p.long_fillers(false), p.long_fillers(true)), (p.errors(false), p.errors(true))] {
                assert_eq!(i.len(), we.len());
                assert!(we.iter().all(|l| !singular(l)), "{we:?}");
            }
        }
        assert_eq!(REFUSALS.len(), WE_REFUSALS.len());
        assert!(SHORT_FILLERS.iter().all(|l| !singular(l)));
        w.we = true;
        assert!(w.refusals().iter().chain([&w.no_key()]).all(|l| !singular(l)));
        assert_eq!(w.fill(Persona::ModernFemale.greetings(true)[0]), "Hi, Sam. We're Vic. Hold the space bar or type whenever you're ready.");
        let p = system_prompt(&w, "");
        assert!(p.contains("Always refer to yourself in the plural") && p.contains("set_we"));
        w.we = false;
        assert!(singular(w.no_key()));
        assert!(system_prompt(&w, "").contains("in the singular: I, me, my"));
        assert!(singular("I'd rather not") && !singular("In it, we're fine."));
    }

    #[test]
    fn personas_differ() {
        assert_ne!(Persona::Male.name_key(), Persona::Female.name_key());
        assert_eq!(Persona::Male.name_key(), "name");
        assert!(Persona::ALL.iter().all(|p| p.image().starts_with(b"\x89PNG")));
        let keys: std::collections::HashSet<_> = Persona::ALL.iter().map(|p| p.name_key()).collect();
        assert_eq!(keys.len(), Persona::ALL.len());
    }

    #[test]
    fn tags_round_trip() {
        for p in Persona::ALL {
            assert_eq!(Persona::from_tag(p.tag()), Some(p));
        }
        assert!(Persona::Male.retro() && Persona::Female.retro());
        assert!(!Persona::ModernMale.retro() && !Persona::ModernFemale.retro());
    }
}
