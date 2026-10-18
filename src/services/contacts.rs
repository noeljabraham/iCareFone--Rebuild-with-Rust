use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::domain::Contact;

pub fn parse_addressbook(db_path: &Path) -> Result<Vec<Contact>, String> {
    if !db_path.exists() {
        return Err(format!("Database file not found: {}", db_path.display()));
    }
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| format!("Failed to open AddressBook.sqlitedb: {err}"))?;

    let mut stmt = connection
        .prepare(
            "SELECT ROWID, First, Last, Organization, JobTitle 
             FROM ABPerson 
             ORDER BY First, Last",
        )
        .or_else(|_| {
            connection.prepare(
                "SELECT ROWID, First, Last, Organization, JobTitle 
                 FROM record 
                 ORDER BY First, Last",
            )
        })
        .map_err(|err| format!("Failed to query contact records: {err}"))?;

    let contact_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            ))
        })
        .map_err(|err| err.to_string())?;

    let mut contacts = Vec::new();
    let mut id_map = HashMap::new();

    for (idx, row) in contact_rows.enumerate() {
        if let Ok((id, first, last, org, job)) = row {
            id_map.insert(id, idx);
            contacts.push(Contact {
                id,
                first_name: first,
                last_name: last,
                phone_numbers: Vec::new(),
                emails: Vec::new(),
                organization: org,
                job_title: job,
            });
        }
    }

    // Read phone numbers and emails from ABMultiValue
    if let Ok(mut mv_stmt) = connection.prepare(
        "SELECT record_id, property, value FROM ABMultiValue WHERE value IS NOT NULL",
    ) {
        let mv_rows = mv_stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?, // 3 = Phone, 4 = Email in standard iOS AddressBook
                row.get::<_, String>(2)?,
            ))
        });

        if let Ok(rows) = mv_rows {
            for item in rows.flatten() {
                let (record_id, prop, val) = item;
                if let Some(&contact_idx) = id_map.get(&record_id) {
                    if let Some(contact) = contacts.get_mut(contact_idx) {
                        if prop == 3 {
                            contact.phone_numbers.push(val);
                        } else if prop == 4 {
                            contact.emails.push(val);
                        }
                    }
                }
            }
        }
    }

    Ok(contacts)
}

pub fn export_vcards(contacts: &[Contact], destination: &Path) -> Result<String, String> {
    let mut all_vcards = String::new();
    for contact in contacts {
        all_vcards.push_str(&contact.to_vcard());
        all_vcards.push_str("\r\n");
    }
    fs::write(destination, all_vcards)
        .map_err(|err| format!("Failed to write vCard export: {err}"))?;
    Ok(format!(
        "Successfully exported {} contacts to {}",
        contacts.len(),
        destination.display()
    ))
}

