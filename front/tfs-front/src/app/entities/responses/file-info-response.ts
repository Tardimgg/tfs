import {UserTokens} from '../user_token';
import {range} from 'rxjs';


export interface NodeMeta {
  File: FileMeta,
  Folder: FolderMeta,
}

export interface FileMetaData {
  hash:          string;
  hash_local_id: string;
}
export interface FileMeta {
  path: string,
  data: Array<Array< number[] | FileMetaData>>;
}

export interface DataMetaKey {
  hash: string,
  hash_local_id: string
}


export class DataSource {
  range: [number, number];
  key: DataMetaKey


  constructor(range: [number, number], key: DataMetaKey) {
    this.range = range;
    this.key = key;
  }
}

export interface DataMeta {
  key: DataMetaKey,
  data_type: DataNodeType
}

export enum DataNodeType {
  Leaf = "Leaf",
  Parent = "Parent"
}

export interface DataNodeLeaf {

}

export interface DataNodeParent {

}

export interface FolderMeta {

}

