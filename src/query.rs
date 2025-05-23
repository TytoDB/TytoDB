use std::{fs::File, io::{self, Error, ErrorKind},collections::HashMap, os::unix::fs::{FileExt, MetadataExt}, sync::Arc};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::{container::Container, database::{generate_secure_code, Database}, gerr, lexer_functions::{AlbaTypes, Token}, query_conditions::QueryConditions, row::Row};


const PAGE_SIZE: usize = 100;

type QueryPage = (Vec<u64>, String);
pub type PrimitiveQueryConditions = (Vec<(Token, Token, Token)>, Vec<(usize, char)>);

type Rows = (Vec<String>, Vec<Vec<AlbaTypes>>);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Query {
    pub rows: Rows,
    pub pages: Vec<QueryPage>,
    pub current_page: usize,
    pub column_names: Vec<String>,
    pub column_types: Vec<AlbaTypes>,
    pub id: String,
}

impl Query {
    pub fn duplicate(&self) -> Self {
        
        Query {
            rows: self.rows.clone(),
            pages: self.pages.clone(),
            current_page: self.current_page, 
            column_names: self.column_names.clone(),
            column_types: self.column_types.clone(),
            id: self.id.clone(),
        }
    }

    pub fn trim(&mut self) {
        
        self.column_types = self
            .column_types
            .iter()
            .filter(|p| !matches!(p, AlbaTypes::NONE))
            .cloned()
            .collect();
        self.column_names = self
            .column_names
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect();
        
    }

    pub fn new(column_types: Vec<AlbaTypes>) -> Self {
        
        let mut n = Query {
            rows: (Vec::new(), Vec::new()), 
            pages: Vec::new(), 
            current_page: 0, 
            column_names: Vec::new(), 
            column_types,
            id: generate_secure_code(100),
        };
        n.trim();
        
        n
    }

    pub fn new_none(column_types: Vec<AlbaTypes>) -> Self {
        
        let mut a = Query {
            rows: (Vec::new(), Vec::new()), 
            pages: Vec::new(), 
            current_page: 0, 
            column_names: Vec::new(), 
            column_types,
            id: "".to_string(),
        };
        a.trim();
        
        a
    }

    pub fn join(&mut self, foreign: Query) {
        if foreign.column_types != self.column_types {
            return;
        }
        
        for (idx, (foreign_ids, container_name)) in foreign.pages.into_iter().enumerate() {
            if idx < self.pages.len() {
                let (self_ids, _) = &mut self.pages[idx];
                
                for foreign_id in foreign_ids {
                    if self_ids.len() < PAGE_SIZE {
                        self_ids.push(foreign_id);
                    } else {
                        break;
                    }
                }
            } else {
                self.pages.push((foreign_ids, container_name));
            }
        }
        
        self.trim();
    }

    pub async fn load_rows(&mut self, database: &mut Database) -> Result<(), Error> {
        
        
        if self.pages.is_empty() {
            
            return Ok(());
        }
        
        
        let page = match self.pages.get(self.current_page) {
            Some(a) => {
                
                a
            },
            None => {
                
                return Err(gerr("There is no page"))
            }
        };
        
        let container_name = &page.1;
        
        let container = match database.container.get(container_name) {
            Some(a) => {
                
                a.read().await
            },
            None => {
                
                return Err(gerr(&format!("There is no container in the given database named {}", container_name)))
            }
        };
        
        let mut rows = Vec::new();
        
        for i in page.0.iter() {
            
            let indexes = (*i, *i + 1);
            println!("load_rows: {:?}",indexes);
            match container.get_rows(indexes).await?.get(0) {
                Some(a) => {
                    rows.push(a.clone());
                },
                None => {
                    
                    continue;
                }
            }
        }
        
        
        self.rows = (container.column_names(), rows);
        self.trim();
        Ok(())
    }

    pub async fn next(&mut self, database: &mut Database) -> Result<(), Error> {
        
        
        if self.pages.is_empty() {
            
            return Ok(());
        }
        
        
        if self.current_page + 1 >= self.pages.len() {
            
            return Ok(());
        }
        
        self.current_page += 1;
        
        
        
        self.load_rows(database).await?;
        self.trim();
        
        
        Ok(())
    }

    pub async fn previous(&mut self, database: &mut Database) -> Result<(), Error> {
        
        
        if self.pages.is_empty() {
            
            return Ok(());
        }
        
        
        if self.current_page == 0 {
            
            return Ok(());
        }
        
        self.current_page -= 1;
        
        
        
        self.load_rows(database).await?;
        self.trim();
        
        
        Ok(())
    }

    pub fn push(&mut self, subject: (Vec<u64>, String)) {
        
        self.pages.push(subject);
        
    }
}

pub struct SearchArguments {
    element_size : usize,
    header_offset : usize,
    file : Arc<RwLock<File>>,
    container_headers : HashMap<String,AlbaTypes>,
    container_values : Vec<(String,AlbaTypes)>,
    container_name : String,
    conditions : QueryConditions

}
const CHUNK_MATRIX : usize = 4096 * 10;

pub async fn search(container : Arc<Container>,args : SearchArguments) -> Result<Query,Error>{
    let element_size = args.element_size;
    let header_offset = args.header_offset;

    let file = args.file.read().await;
    let file_size = file.metadata()?.size() as usize;
    let total_rows = (file_size-header_offset)/element_size;
    let mut readen_rows = 0;
    let rows_per_iteration = std::cmp::max(1, CHUNK_MATRIX / element_size).min(total_rows);

    

    let mut rows : Vec<(Row,usize)> = Vec::new();
    while readen_rows < total_rows{
        let to_read = rows_per_iteration.min(total_rows-readen_rows);
        let read_size = to_read * element_size;
        let mut buffer = vec![0u8;read_size];
        file.read_exact_at(&mut buffer, (header_offset + (readen_rows * element_size)) as u64)?;
        for i in 0..to_read{
            let buff = &buffer[(i*element_size)..((i+1)*element_size)];
            let row = match container.deserialize_row(buff).await{
                Ok(row_content) => {
                    let mut data : HashMap<String,AlbaTypes> = HashMap::new();
                    for (index,value) in container.headers.iter().enumerate(){
                        let column_value = match row_content.get(index){
                            Some(a) => {
                                let cv = a.to_owned();
                                if std::mem::discriminant(&cv) != std::mem::discriminant(&value.1){
                                    return Err(gerr("Invalid alba type row order, unmatching stuff"))
                                }
                                cv
                            },
                            None => {
                                return Err(gerr("Invalid alba type row order, missing stuff"));
                            }
                        };
                        data.insert(value.0.clone(),column_value);
                    }
                    Row{
                        data,
                        metadata:args.container_headers.clone()
                    }
                },
                Err(e) => {
                    return Err(e)
                }
            };
            rows.push((row,readen_rows+i));

        }
        readen_rows += 1;
    }
    
    let mut query = Query::new(args.container_values.iter().map(|f|f.1.clone()).collect());
    let mut page_bucket : Vec<u64> = Vec::with_capacity(100);
    let mut page_bucket_len = 0;

    for i in rows{
        if args.conditions.row_match(&i.0)?{
            page_bucket.push(i.1 as u64); page_bucket_len += 1;
            if page_bucket_len >= 100{
                query.push((page_bucket.clone(),args.container_name.clone()));
                page_bucket.clear(); page_bucket_len = 0;
            }
        }
    }
    if page_bucket_len > 0 {
        query.push((page_bucket.clone(),args.container_name.clone()));
    }
    drop(page_bucket);
    drop(page_bucket_len);

    Ok(query)
}