//! Configuration parser for COLMAP and OpenMVS help output.
//!
//! This module provides functionality to extract configuration from tool help output
//! and convert them into structured schemas that can be used throughout the application.

use crate::runtimes::{Runtime, RuntimeFactory};
use colmap_openmvs_api::{
    ConfigParameter, ConfigSchema, EnvVarConfig, EnvVarWithHelp, LoadedProjectConfig,
    SavedProjectConfig, ToolConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use tokio::io::AsyncReadExt;

/// Root schema for the YAML help output
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HelpSchema {
    tools: ToolsSchema,
    environment_variables: Vec<String>,
}

/// Tools container
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolsSchema {
    colmap: BTreeMap<String, CommandHelp>,
    openmvs: BTreeMap<String, CommandHelp>,
}

/// Individual command help
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandHelp {
    help: String,
    environment_variables: Vec<String>,
}

/// Parse configuration from a prepared container image by running help commands.
pub async fn get_image_config(image_tag: String) -> anyhow::Result<ConfigSchema> {
    let rt = RuntimeFactory::proot().await;

    // First, try to get the image build date from the list of prepared images
    let build_date = match rt.list_images().await {
        Ok(images) => images
            .iter()
            .find(|img| img.tag_str() == image_tag)
            .and_then(|img| img.build_date.clone()),
        Err(_) => None,
    };

    // Get help output from colmap
    let colmap_help = get_tool_help(&rt, &image_tag, "colmap").await?;
    let colmap_schema: HelpSchema = serde_saphyr::from_str::<HelpSchema>(&colmap_help)
        .map_err(|e| anyhow::anyhow!("Failed to parse colmap help output: {}", e))?;

    // Get help output from openmvs
    let openmvs_help = get_tool_help(&rt, &image_tag, "openmvs").await?;
    let openmvs_schema: HelpSchema = serde_saphyr::from_str::<HelpSchema>(&openmvs_help)
        .map_err(|e| anyhow::anyhow!("Failed to parse openmvs help output: {}", e))?;

    // Extract tools and build help text map
    let mut all_tools = Vec::new();
    let mut help_map: HashMap<String, String> = HashMap::new();

    // Process COLMAP
    for (cmd_name, cmd_help) in &colmap_schema.tools.colmap {
        let parameters = parse_help_text(&cmd_help.help)?;
        all_tools.push(ToolConfig {
            tool: "colmap".to_string(),
            command: cmd_name.clone(),
            parameters,
            environment_variables: cmd_help.environment_variables.clone(),
        });

        // Build help map for matching env vars
        for env_var in &cmd_help.environment_variables {
            if !help_map.contains_key(env_var) {
                help_map.insert(env_var.clone(), cmd_help.help.clone());
            }
        }
    }

    // Process OpenMVS
    for (cmd_name, cmd_help) in &openmvs_schema.tools.openmvs {
        let parameters = parse_help_text(&cmd_help.help)?;
        all_tools.push(ToolConfig {
            tool: "openmvs".to_string(),
            command: cmd_name.clone(),
            parameters,
            environment_variables: cmd_help.environment_variables.clone(),
        });

        // Build help map for matching env vars
        for env_var in &cmd_help.environment_variables {
            if !help_map.contains_key(env_var) {
                help_map.insert(env_var.clone(), cmd_help.help.clone());
            }
        }
    }

    // Build environment_variables list from top-level, preserving order
    // Merge both colmap and openmvs top-level env vars
    let environment_variables: Vec<EnvVarWithHelp> = colmap_schema
        .environment_variables
        .iter()
        .chain(openmvs_schema.environment_variables.iter())
        .map(|name| {
            let help = help_map.get(name).cloned();
            EnvVarWithHelp {
                name: name.clone(),
                help,
            }
        })
        .collect();

    Ok(ConfigSchema {
        image_tag,
        build_date,
        tools: all_tools,
        environment_variables,
    })
}

/// Run a tool's help command inside the container and capture output.
async fn get_tool_help(
    rt: &dyn crate::runtimes::Runtime,
    image_tag: &str,
    tool: &str,
) -> anyhow::Result<String> {
    let args = vec![tool.to_string(), "--help".to_string()];

    let mut handle = rt.run(image_tag, &args, &[], &[]).await?;

    let stdout = handle
        .take_stdout()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout from {} --help", tool))?;

    let stderr = handle
        .take_stderr()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr from {} --help", tool))?;

    // Read from stdout
    let mut output = Vec::new();
    let mut stdout_reader = tokio::io::BufReader::new(stdout);
    stdout_reader.read_to_end(&mut output).await?;

    // Also read stderr in case help is written there
    let mut stderr_output = Vec::new();
    let mut stderr_reader = tokio::io::BufReader::new(stderr);
    stderr_reader.read_to_end(&mut stderr_output).await?;

    // Wait for the process to finish and check exit code
    let status = handle.wait().await?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let stdout_str = String::from_utf8_lossy(&output);
        let stderr_str = String::from_utf8_lossy(&stderr_output);
        return Err(anyhow::anyhow!(
            "{} --help failed with exit code {}\nstdout:\n{}\nstderr:\n{}",
            tool,
            code,
            stdout_str,
            stderr_str
        ));
    }

    let stdout_str = String::from_utf8_lossy(&output).to_string();
    let stderr_str = String::from_utf8_lossy(&stderr_output).to_string();

    // Combine outputs, preferring stdout
    let combined = if !stdout_str.is_empty() {
        stdout_str
    } else {
        stderr_str
    };

    Ok(combined)
}

/// Parse help text (plain text with parameters) to extract parameters.
fn parse_help_text(help_text: &str) -> anyhow::Result<Vec<ConfigParameter>> {
    let mut parameters = Vec::new();

    let lines: Vec<&str> = help_text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Parse parameter lines (lines starting with - or --)
        if trimmed.starts_with('-') || trimmed.starts_with("--") {
            if let Some(param) = parse_parameter_line(trimmed) {
                parameters.push(param);
            }
        }

        i += 1;
    }

    Ok(parameters)
}

/// Parse a single parameter line and extract its components.
fn parse_parameter_line(line: &str) -> Option<ConfigParameter> {
    let line = line.trim();

    // Try to match parameter patterns
    // Handle short and long form parameters: -h [ --help ]
    if line.contains('[') && line.contains(']') {
        return parse_bracketed_parameter(line);
    }

    // Handle standard parameter lines
    // Extract parameter name, default value, enum values, and description
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let param_name = parts[0].to_string();

    // Skip if it's not a valid parameter
    if !param_name.starts_with('-') && !param_name.contains('.') {
        return None;
    }

    let mut default_value = None;
    let mut enum_values = Vec::new();
    let mut description = String::new();

    // Parse the rest of the line for default values, enums, and description
    let rest = line[param_name.len()..].trim();

    // Check for arg keyword
    let rest = if let Some(stripped) = rest.strip_prefix("arg") {
        stripped.trim()
    } else {
        rest
    };

    // Check for default value pattern: (=value)
    if let Some(default_match) = rest.find('(') {
        if let Some(close_paren) = rest.find(')') {
            let default_section = &rest[default_match + 1..close_paren];
            if default_section.trim_start().starts_with('=') {
                let value = default_section[1..].trim();
                if !value.is_empty() {
                    default_value = Some(value.to_string());
                }
            }
        }
    }

    // Check for enum values pattern: {value1, value2, ...}
    if let Some(brace_start) = rest.find('{') {
        if let Some(brace_end) = rest.find('}') {
            let enum_section = &rest[brace_start + 1..brace_end];
            enum_values = enum_section
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    // Extract description (text after default and enum patterns)
    let mut desc_start = 0;
    if let Some(last_paren) = rest.rfind(')') {
        desc_start = last_paren + 1;
    } else if let Some(last_brace) = rest.rfind('}') {
        desc_start = last_brace + 1;
    }

    if desc_start < rest.len() {
        description = rest[desc_start..].trim().to_string();
    }

    Some(ConfigParameter {
        name: param_name,
        description,
        default_value,
        enum_values,
    })
}

/// Parse parameters with bracketed alternatives like: -h [ --help ]
fn parse_bracketed_parameter(line: &str) -> Option<ConfigParameter> {
    let line = line.trim();

    // Extract the main parameter name (before any brackets)
    let parts: Vec<&str> = line.split(['[', ']']).collect();
    if parts.is_empty() {
        return None;
    }

    let param_name = parts[0].trim().to_string();
    if !param_name.starts_with('-') {
        return None;
    }

    // Extract description (everything after the closing bracket)
    let description = if parts.len() > 2 {
        parts[2..].join("]").trim().to_string()
    } else {
        String::new()
    };

    Some(ConfigParameter {
        name: param_name,
        description,
        default_value: None,
        enum_values: Vec::new(),
    })
}

/// Load and parse project configuration from config.sh in a project directory.
pub async fn load_project_config(project_path: String) -> anyhow::Result<LoadedProjectConfig> {
    let project_path = Path::new(&project_path);

    if !project_path.exists() {
        return Err(anyhow::anyhow!(
            "Project path does not exist: {}",
            project_path.display()
        ));
    }

    let config_path = project_path.join("config.sh");

    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "config.sh not found in project: {}",
            config_path.display()
        ));
    }

    let content = tokio::fs::read_to_string(&config_path).await?;

    // Parse the config.sh file
    let mut image_tag = String::new();
    let mut environment_variables = Vec::new();
    let mut custom_script = String::new();
    let mut in_env_vars = false;
    let mut in_custom_script = false;

    for line in content.lines() {
        // Extract image tag from comment
        if line.trim_start().starts_with("# Generated from image:") {
            if let Some(tag) = line.split("# Generated from image:").nth(1) {
                image_tag = tag.trim().to_string();
            }
        }

        // Check for custom script markers
        if line.trim() == "# ===== BEGIN CUSTOM SCRIPT =====" {
            in_custom_script = true;
            continue;
        }
        if line.trim() == "# ===== END CUSTOM SCRIPT =====" {
            in_custom_script = false;
            continue;
        }

        // Capture custom script if we're in the marked section
        if in_custom_script {
            custom_script.push_str(line);
            custom_script.push('\n');
            continue;
        }

        // Parse environment variables
        if line.trim_start().starts_with("export ") {
            in_env_vars = true;
            if let Some(env_line) = parse_env_var_line(line) {
                environment_variables.push(env_line);
            }
        } else if in_env_vars && !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            // End of environment variables section
            in_env_vars = false;
        }
    }

    // Clean up trailing newline from custom script
    if custom_script.ends_with('\n') {
        custom_script.pop();
    }

    Ok(LoadedProjectConfig {
        image_tag,
        environment_variables,
        custom_script,
    })
}

/// Parse a single environment variable export line
fn parse_env_var_line(line: &str) -> Option<EnvVarConfig> {
    let line = line.trim();
    if !line.starts_with("export ") {
        return None;
    }

    let rest = &line[7..]; // Skip "export "

    // Find the equals sign
    if let Some(eq_pos) = rest.find('=') {
        let name = rest[..eq_pos].trim().to_string();
        let value_part = &rest[eq_pos + 1..];

        // Parse the value, handling both single and double quotes
        let value = if value_part.starts_with('\'') && value_part.len() > 1 {
            // Single quoted - find closing quote
            if let Some(close_pos) = value_part[1..].find('\'') {
                value_part[1..close_pos + 1].to_string()
            } else {
                return None;
            }
        } else if value_part.starts_with('"') && value_part.len() > 1 {
            // Double quoted - find closing quote and unescape
            if let Some(close_pos) = value_part[1..].find('"') {
                let raw_value = &value_part[1..close_pos + 1];
                // Unescape double-quoted string
                raw_value
                    .replace("\\\\", "\\")
                    .replace("\\\"", "\"")
                    .replace("\\$", "$")
                    .replace("\\`", "`")
            } else {
                return None;
            }
        } else {
            // Unquoted value (shouldn't normally happen but handle it)
            value_part.trim().to_string()
        };

        return Some(EnvVarConfig { name, value });
    }

    None
}

/// Save environment variable configuration to config.sh in a project directory.
pub async fn save_project_config(
    project_path: String,
    config: SavedProjectConfig,
) -> anyhow::Result<()> {
    let project_path = Path::new(&project_path);

    if !project_path.exists() {
        return Err(anyhow::anyhow!(
            "Project path does not exist: {}",
            project_path.display()
        ));
    }

    let mut script_content = String::new();
    script_content.push_str("#!/bin/bash\n");
    script_content.push_str("# Configuration for COLMAP/OpenMVS pipeline\n");
    script_content.push_str(&format!("# Generated from image: {}\n", config.image_tag));
    script_content.push_str(&format!(
        "# Generated at: {}\n",
        chrono::Local::now().to_rfc3339()
    ));
    script_content.push_str("#\n");
    script_content.push_str("# Usage: source ./config.sh\n");
    script_content.push_str("# Then run your COLMAP/OpenMVS commands\n");
    script_content.push('\n');

    if config.environment_variables.is_empty() {
        script_content.push_str("# No environment variables configured\n");
    } else {
        script_content.push_str("# Environment variables\n");
        for env_var in &config.environment_variables {
            let escaped_value = if env_var.value.contains('\'') {
                format!(
                    "\"{}\"",
                    env_var
                        .value
                        .replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('$', "\\$")
                        .replace('`', "\\`")
                )
            } else {
                format!("'{}'", env_var.value)
            };
            script_content.push_str(&format!("export {}={}\n", env_var.name, escaped_value));
        }
    }

    // Add custom script section if provided
    if let Some(custom_script) = &config.custom_script {
        if !custom_script.is_empty() {
            script_content.push_str("\n# ===== BEGIN CUSTOM SCRIPT =====\n");
            script_content.push_str(custom_script);
            if !custom_script.ends_with('\n') {
                script_content.push('\n');
            }
            script_content.push_str("# ===== END CUSTOM SCRIPT =====\n");
        }
    }

    script_content.push_str("\necho \"Configuration loaded from $0\"\n");

    let config_path = project_path.join("config.sh");
    tokio::fs::write(&config_path, &script_content).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&config_path, perms)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_parameters() {
        let help_text = r#"
  --project_path arg                Paths to the reconstruction projects.
  -h [ --help ]                     Show help message.
  --quality arg (=high)             Quality level
  --log_level arg (=0)              Logging level
"#;

        let params = parse_help_text(help_text).unwrap();
        assert!(!params.is_empty());
        assert!(params.iter().any(|p| p.name == "--project_path"));
        assert!(params.iter().any(|p| p.name == "-h"));
    }

    #[test]
    fn test_parameter_with_default_and_enums() {
        let line = "  --quality arg (=high) {low, medium, high, extreme}   Quality level";
        let param = parse_parameter_line(line);
        assert!(param.is_some());

        let param = param.unwrap();
        assert_eq!(param.name, "--quality");
        assert_eq!(param.default_value, Some("high".to_string()));
        assert!(param.enum_values.contains(&"medium".to_string()));
        assert!(param.enum_values.contains(&"low".to_string()));
    }

    #[test]
    fn test_bracketed_parameter() {
        let line = "-h [ --help ]                     Show help message.";
        let param = parse_bracketed_parameter(line);
        assert!(param.is_some());

        let param = param.unwrap();
        assert_eq!(param.name, "-h");
        assert_eq!(param.description.trim(), "Show help message.");
    }

    #[test]
    fn test_parameter_with_default() {
        let line = "  --log_level arg (=0)              Logging level";
        let param = parse_parameter_line(line);
        assert!(param.is_some());

        let param = param.unwrap();
        assert_eq!(param.name, "--log_level");
        assert_eq!(param.default_value, Some("0".to_string()));
    }

    #[test]
    fn test_dotted_parameter() {
        let line = "  Mapper.min_matches arg (=4)       Minimum number of matches";
        let param = parse_parameter_line(line);
        assert!(param.is_some());

        let param = param.unwrap();
        assert_eq!(param.name, "Mapper.min_matches");
        assert_eq!(param.default_value, Some("4".to_string()));
    }
}
