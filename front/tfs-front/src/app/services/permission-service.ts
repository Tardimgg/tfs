import {Injectable} from '@angular/core';
import {HttpClient} from '@angular/common/http';
import {AuthService} from './auth.service';
import {PermissionType} from '../entities/permission-type';
import {ObjType} from '../entities/obj-type';
import {UserPermission} from '../entities/user-permission';

@Injectable({
  providedIn: 'root'
})
export class PermissionService {

  // private url = 'http://127.0.0.1:8080/auth/api/';
  // private url = document.location.hostname + '/api/auth/api/';
  // private url = 'http://127.0.0.1:8080/virtual_fs';
  // private url = 'https://10.42.0.212:8081';
  private url = 'https://158.160.98.131:8081';
  
  constructor(private client: HttpClient, private authService: AuthService) { }


  public putPermission(login: string, objType: ObjType, objId: string, permission: PermissionType) {
    let token = this.authService.getToken();

    // console.log(PermissionType as any);
    // let permissionString = (PermissionType as any).entru[permission] as string;

    this.client.put(`${this.url}/permission/user/${login}/${objType}/${objId}/${permission}`, {}, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    }).subscribe(
      (r)=>{console.log('got r', r)}
    )
  }


  public deletePermission(login: string, objType: ObjType, objId: string, permission: PermissionType) {
    let token = this.authService.getToken();

    this.client.delete(`${this.url}/permission/user/${login}/${objType}/${objId}/${permission}`, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    }).subscribe(
      (r)=>{console.log('got r', r)}
    )
  }

  public async getPermissions(objType: ObjType, objId: string) {
    let token = this.authService.getToken();

    let response = await fetch(`${this.url}/permission/user/all/${objType}/${objId}`, {
      headers: {
        "Authorization": token == null ? "" : token
      }
    });


    let json = await response.json();
    return json as UserPermission[];
  }
}


export function castToE(permissionType: PermissionType) {
  switch (permissionType) {
    case PermissionType.Read:
      return "Читать";
    case PermissionType.Write:
      return "Изменять";
    case PermissionType.GrantRead:
      return "Управлять правами на чтение";
    case PermissionType.GrantWrite:
      return "Управлять правами на изменение";
    case PermissionType.ReadRecursively:
      return "Читать всю иерархию";
    case PermissionType.WriteRecursively:
      return "Изменять всю иерархию";
    case PermissionType.GrantReadRecursively:
      return "Управлять правами на чтение иерархии";
    case PermissionType.GrantWriteRecursively:
      return "Управлять правами на изменение иерархии";
    case PermissionType.All:
      return "Владелец";
  }
}

